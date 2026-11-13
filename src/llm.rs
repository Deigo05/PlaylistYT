//! Cliente LLM intercambiable: APIs compatibles con OpenAI (DeepSeek, OpenAI, Gemini,
//! OpenRouter, Ollama…) y la Messages API nativa de Claude.

use crate::config::{LlmConfig, Provider};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;

const ANTHROPIC_VERSION: &str = "2026-10-21";
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub struct LlmClient {
    http: reqwest::Client,
    config: LlmConfig,
    api_key: String,
}

impl LlmClient {
    pub fn new(config: LlmConfig, api_key: String) -> Result<Self> {
        if config.provider.needs_api_key() && api_key.trim().is_empty() {
            bail!(
                "Falta la API key de {}. Añádela en Ajustes.",
                config.provider.label()
            );
        }
        if config.model.trim().is_empty() {
            bail!("Falta el nombre del modelo en Ajustes.");
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            config,
            api_key: api_key.trim().to_string(),
        })
    }

    pub fn provider_label(&self) -> &'static str {
        self.config.provider.label()
    }

    /// Envía un prompt y devuelve la respuesta como JSON. `schema` es un JSON Schema
    /// (objetos con `additionalProperties: false` y `required`) que se usa como formato
    /// estructurado en Claude y como guía dentro del prompt en el resto.
    pub async fn chat_json(&self, system: &str, user: &str, schema: &Value) -> Result<Value> {
        match self.config.provider {
            Provider::Claude => self.anthropic(system, user, schema).await,
            _ => self.openai_compatible(system, user, schema).await,
        }
    }

    async fn openai_compatible(&self, system: &str, user: &str, schema: &Value) -> Result<Value> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let system = format!(
            "{system}\n\nResponde ÚNICAMENTE con un objeto JSON válido que cumpla este JSON Schema, sin texto adicional:\n{}",
            serde_json::to_string(schema)?
        );
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": 8000,
            "stream": false,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });

        let mut response = self.post_openai(&url, &body).await?;
        // Algunos proveedores no aceptan `response_format`: se reintenta sin él.
        if matches!(response.status().as_u16(), 400 | 422) {
            body.as_object_mut().unwrap().remove("response_format");
            response = self.post_openai(&url, &body).await?;
        }
        let value = read_json_response(response, self.provider_label()).await?;
        let text = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("Respuesta sin contenido: {value}"))?;
        extract_json(text)
    }

    async fn post_openai(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        let mut req = self.http.post(url).json(body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        req.send()
            .await
            .with_context(|| format!("No se pudo conectar con {}", self.provider_label()))
    }

    async fn anthropic(&self, system: &str, user: &str, schema: &Value) -> Result<Value> {
        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
        let body = json!({
            "model": self.config.model,
            "max_tokens": 16000,
            "system": system,
            "messages": [{"role": "user", "content": user}],
            "output_config": {
                "effort": "low",
                "format": {"type": "json_schema", "schema": schema},
            },
            "fallbacks": "default",
        });
        let response = self
            .http
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", ANTHROPIC_FALLBACK_BETA)
            .json(&body)
            .send()
            .await
            .context("No se pudo conectar con la API de Claude")?;
        let value = read_json_response(response, self.provider_label()).await?;
        if value["stop_reason"] == "refusal" {
            bail!("Claude rechazó la petición: {}", value["stop_details"]);
        }
        let text = value["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str())
            .ok_or_else(|| anyhow!("Respuesta de Claude sin texto: {value}"))?;
        extract_json(text)
    }
}

async fn read_json_response(response: reqwest::Response, provider: &str) -> Result<Value> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .or_else(|| v["error"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or(text);
        let hint = match status.as_u16() {
            401 | 403 => " (revisa la API key en Ajustes)",
            404 => " (revisa la URL base y el modelo en Ajustes)",
            429 => " (límite de uso o saldo del proveedor)",
            _ => "",
        };
        bail!("{provider} respondió {status}{hint}: {detail}");
    }
    serde_json::from_str(&text).with_context(|| format!("{provider} devolvió algo que no es JSON"))
}

/// Extrae un objeto JSON de la respuesta del modelo, tolerando bloques ```json y texto alrededor.
pub fn extract_json(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("El modelo devolvió una respuesta vacía");
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..=end])
    {
        return Ok(v);
    }
    bail!("No se encontró JSON válido en la respuesta del modelo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_directo() {
        assert_eq!(extract_json(r#"{"a":1}"#).unwrap()["a"], 1);
    }

    #[test]
    fn json_con_bloque_de_codigo_y_texto() {
        let text = "Aquí tienes:\n```json\n{\"songs\": [\"x\"]}\n```\nSaludos";
        assert_eq!(extract_json(text).unwrap()["songs"][0], "x");
    }

    #[test]
    fn respuesta_vacia_es_error() {
        assert!(extract_json("   ").is_err());
        assert!(extract_json("sin json").is_err());
    }
}

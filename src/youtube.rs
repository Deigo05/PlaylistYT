//! YouTube Data API v3: OAuth de escritorio, crear playlists y añadir vídeos.

use crate::config::Config;
use crate::quota::{COST_INSERT, COST_LIST, Quota};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use yup_oauth2::{InstalledFlowAuthenticator, InstalledFlowReturnMethod};

const SCOPES: &[&str] = &["https://www.googleapis.com/auth/youtube"];
const API: &str = "https://www.googleapis.com/youtube/v3";

/// Abre el navegador con la pantalla de consentimiento de Google en lugar de imprimir la URL.
struct BrowserDelegate;

impl InstalledFlowDelegate for BrowserDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            open::that(url).map_err(|e| format!("no se pudo abrir el navegador: {e}"))?;
            Ok(String::new())
        })
    }
}

#[derive(Debug)]
pub struct QuotaExceeded;

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "YouTube indica que la cuota diaria está agotada. Se reinicia a medianoche (hora del Pacífico)."
        )
    }
}

impl std::error::Error for QuotaExceeded {}

pub struct YouTube {
    http: reqwest::Client,
    token: String,
    quota: Arc<Mutex<Quota>>,
}

impl YouTube {
    /// Obtiene un token de acceso. La primera vez (o si caducó la autorización) abre el
    /// navegador para que el usuario acepte; después usa el token guardado en disco.
    pub async fn connect(config: &Config, quota: Arc<Mutex<Quota>>) -> Result<Self> {
        if config.client_secret_path.trim().is_empty() {
            bail!("Falta el archivo client_secret.json de Google. Configúralo en Ajustes.");
        }
        let secret = yup_oauth2::read_application_secret(config.client_secret_path.trim())
            .await
            .context("No se pudo leer el client_secret.json (¿es un cliente OAuth de tipo \"App de escritorio\"?)")?;
        let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
            .persist_tokens_to_disk(Config::tokens_path())
            .flow_delegate(Box::new(BrowserDelegate))
            .build()
            .await
            .context("No se pudo iniciar la autorización con Google")?;
        let token = auth
            .token(SCOPES)
            .await
            .context("Google no autorizó el acceso a YouTube")?;
        let token = token
            .token()
            .ok_or_else(|| anyhow!("Google no devolvió un token de acceso"))?
            .to_string();
        Ok(Self {
            http: reqwest::Client::new(),
            token,
            quota,
        })
    }

    async fn call(&self, req: reqwest::RequestBuilder, cost: u32) -> Result<Value> {
        // Google cobra la cuota aunque la petición falle.
        self.quota.lock().unwrap().add(cost);
        let response = req
            .bearer_auth(&self.token)
            .send()
            .await
            .context("No se pudo conectar con YouTube")?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(body);
        }
        let reasons: Vec<&str> = body["error"]["errors"]
            .as_array()
            .map(|errs| errs.iter().filter_map(|e| e["reason"].as_str()).collect())
            .unwrap_or_default();
        if reasons.iter().any(|r| *r == "quotaExceeded" || *r == "dailyLimitExceeded") {
            self.quota.lock().unwrap().exhaust();
            return Err(QuotaExceeded.into());
        }
        let message = body["error"]["message"].as_str().unwrap_or("error desconocido");
        bail!("YouTube respondió {status}: {message}")
    }

    /// Nombre del canal de la cuenta conectada.
    pub async fn channel_title(&self) -> Result<String> {
        let req = self
            .http
            .get(format!("{API}/channels"))
            .query(&[("part", "snippet"), ("mine", "true")]);
        let body = self.call(req, COST_LIST).await?;
        Ok(body["items"][0]["snippet"]["title"]
            .as_str()
            .unwrap_or("(cuenta sin canal)")
            .to_string())
    }

    /// Crea una playlist oculta (unlisted) y devuelve su id.
    pub async fn create_playlist(&self, title: &str, description: &str) -> Result<String> {
        let req = self
            .http
            .post(format!("{API}/playlists"))
            .query(&[("part", "snippet,status")])
            .json(&json!({
                "snippet": {"title": title, "description": description},
                "status": {"privacyStatus": "unlisted"},
            }));
        let body = self.call(req, COST_INSERT).await?;
        body["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("YouTube no devolvió el id de la playlist"))
    }

    /// Añade un vídeo al final de la playlist.
    pub async fn add_video(&self, playlist_id: &str, video_id: &str) -> Result<()> {
        let req = self
            .http
            .post(format!("{API}/playlistItems"))
            .query(&[("part", "snippet")])
            .json(&json!({
                "snippet": {
                    "playlistId": playlist_id,
                    "resourceId": {"kind": "youtube#video", "videoId": video_id},
                },
            }));
        self.call(req, COST_INSERT).await.map(|_| ())
    }
}

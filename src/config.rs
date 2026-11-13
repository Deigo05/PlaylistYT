//! Configuración persistente (config.toml) y claves API (Administrador de credenciales de Windows).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "PlaylistYT";

/// Carpeta de datos de la app: %APPDATA%\PlaylistYT
pub fn data_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PlaylistYT");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    DeepSeek,
    OpenAi,
    Gemini,
    OpenRouter,
    Ollama,
    Claude,
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 7] = [
        Provider::DeepSeek,
        Provider::OpenAi,
        Provider::Gemini,
        Provider::OpenRouter,
        Provider::Ollama,
        Provider::Claude,
        Provider::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Provider::DeepSeek => "DeepSeek",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Google Gemini",
            Provider::OpenRouter => "OpenRouter",
            Provider::Ollama => "Ollama (local)",
            Provider::Claude => "Claude (Anthropic)",
            Provider::Custom => "Personalizado (compatible con OpenAI)",
        }
    }

    /// URL base y modelo por defecto de cada proveedor. Ambos son editables en Ajustes.
    pub fn preset(self) -> (&'static str, &'static str) {
        match self {
            Provider::DeepSeek => ("https://api.deepseek.com", "deepseek-flash"),
            Provider::OpenAi => ("https://api.openai.com/v1", "gpt-5-mini"),
            Provider::Gemini => (
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gemini-2.5-flash",
            ),
            Provider::OpenRouter => ("https://openrouter.ai/api/v1", "deepseek/deepseek-chat"),
            Provider::Ollama => ("http://localhost:11434/v1", "llama3.1"),
            Provider::Claude => ("https://api.anthropic.com", "claude-opus-5-5"),
            Provider::Custom => ("http://localhost:8000/v1", ""),
        }
    }

    pub fn needs_api_key(self) -> bool {
        !matches!(self, Provider::Ollama)
    }

    fn keyring_user(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub ytdlp_path: String,
    pub client_secret_path: String,
    /// Carpeta donde se guardan las descargas (una subcarpeta por playlist).
    #[serde(default = "default_download_dir")]
    pub download_dir: String,
    #[serde(default)]
    pub audio_format: AudioFormat,
    /// Ruta de ffmpeg. Vacía = buscarlo automáticamente.
    #[serde(default)]
    pub ffmpeg_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AudioFormat {
    /// MP3 VBR de alta calidad: el más compatible. Necesita ffmpeg.
    #[default]
    Mp3,
    /// AAC tal como lo sirve YouTube. Sin ffmpeg se guarda sin carátula ni etiquetas.
    M4a,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 2] = [AudioFormat::Mp3, AudioFormat::M4a];

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::M4a => "M4A (AAC, calidad original)",
            AudioFormat::Mp3 => "MP3 (máxima compatibilidad)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::M4a => "m4a",
            AudioFormat::Mp3 => "mp3",
        }
    }

    pub fn needs_ffmpeg(self) -> bool {
        self == AudioFormat::Mp3
    }
}

/// %USERPROFILE%\Music\PlaylistYT
fn default_download_dir() -> String {
    dirs::audio_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PlaylistYT")
        .to_string_lossy()
        .into_owned()
}

impl Default for Config {
    fn default() -> Self {
        let (base_url, model) = Provider::DeepSeek.preset();
        Self {
            llm: LlmConfig {
                provider: Provider::DeepSeek,
                base_url: base_url.into(),
                model: model.into(),
            },
            ytdlp_path: "yt-dlp".into(),
            client_secret_path: String::new(),
            download_dir: default_download_dir(),
            audio_format: AudioFormat::default(),
            ffmpeg_path: String::new(),
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        data_dir().join("config.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(Self::path(), text).context("no se pudo guardar config.toml")
    }

    pub fn tokens_path() -> PathBuf {
        data_dir().join("youtube_tokens.json")
    }
}

/// Lee la API key guardada para un proveedor (vacía si no hay).
pub fn load_api_key(provider: Provider) -> String {
    keyring::Entry::new(KEYRING_SERVICE, &provider.keyring_user())
        .and_then(|e| e.get_password())
        .unwrap_or_default()
}

pub fn save_api_key(provider: Provider, key: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &provider.keyring_user())?;
    if key.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    } else {
        entry
            .set_password(key)
            .context("no se pudo guardar la API key en el Administrador de credenciales")
    }
}

//! Playlists creadas por la app, guardadas en history.json (verlas no gasta cuota).

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// Pista de audio original (YouTube Music o canal "Artista - Topic").
    AudioOriginal,
    /// Vídeo titulado "Official Audio" / "Audio oficial".
    AudioOficial,
    /// Videoclip oficial (último recurso).
    VideoOficial,
    Otro,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::AudioOriginal => "Audio original",
            SourceKind::AudioOficial => "Audio oficial",
            SourceKind::VideoOficial => "Videoclip",
            SourceKind::Otro => "Otro",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub song: String,
    pub video_id: String,
    pub video_title: String,
    pub channel: Option<String>,
    pub duration_secs: Option<u32>,
    pub kind: SourceKind,
    /// Nombre limpio "Cantante - Canción" que se usa al descargar (sin extensión).
    /// Vacío en playlists creadas con versiones antiguas de la app.
    #[serde(default)]
    pub file_name: String,
}

impl PlaylistItem {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }

    /// "Cantante - Canción": el nombre guardado o, si no hay, uno construido con la lista.
    pub fn clean_name(&self, artist: &str) -> String {
        let name = self.file_name.trim();
        if name.is_empty() {
            format!("{artist} - {}", self.song)
        } else {
            name.to_string()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistResult {
    pub playlist_id: String,
    pub title: String,
    pub artist: String,
    pub created_at: DateTime<Local>,
    pub items: Vec<PlaylistItem>,
    /// Canciones pedidas que no se pudieron encontrar o añadir.
    pub missing: Vec<String>,
}

impl PlaylistResult {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/playlist?list={}", self.playlist_id)
    }
}

fn path() -> PathBuf {
    crate::config::data_dir().join("history.json")
}

/// Más recientes primero.
pub fn load() -> Vec<PlaylistResult> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(history: &[PlaylistResult]) {
    if let Ok(text) = serde_json::to_string_pretty(history) {
        let _ = std::fs::write(path(), text);
    }
}

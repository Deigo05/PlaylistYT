//! Descarga con yt-dlp del audio de una playlist creada por la app y limpieza de los
//! nombres de archivo con el LLM ("{cantante} - {canción}").
//!
//! Cada playlist va a su propia carpeta. Un manifiesto (`.playlistyt.json`) recuerda qué
//! archivo corresponde a cada vídeo, para no repetir descargas y poder renombrar.

use crate::agent::Progress;
use crate::config::Config;
use crate::history::{PlaylistItem, PlaylistResult};
use crate::llm::LlmClient;
use crate::{ffmpeg, ytdlp};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const MANIFEST: &str = ".playlistyt.json";
/// Límite de caracteres del nombre (sin extensión), lejos del máximo de Windows.
const MAX_NAME_CHARS: usize = 150;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// video_id → nombre del archivo dentro de la carpeta de la playlist.
    files: BTreeMap<String, String>,
}

impl Manifest {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(MANIFEST))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(self)?)
            .context("no se pudo guardar el registro de descargas")
    }

    /// Archivo ya descargado de ese vídeo, si sigue en la carpeta.
    fn file(&self, dir: &Path, video_id: &str) -> Option<PathBuf> {
        let path = dir.join(self.files.get(video_id)?);
        path.is_file().then_some(path)
    }

    /// Cuántas canciones de la playlist están descargadas.
    pub fn downloaded(&self, dir: &Path, items: &[PlaylistItem]) -> usize {
        items.iter().filter(|it| self.file(dir, &it.video_id).is_some()).count()
    }
}

/// Carpeta de una playlist dentro de la carpeta de descargas.
pub fn playlist_dir(base: &str, playlist: &PlaylistResult) -> PathBuf {
    PathBuf::from(base.trim()).join(sanitize_file_name(&playlist.title))
}

/// Quita los caracteres que Windows no admite en nombres de archivo.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > MAX_NAME_CHARS {
        out = out.chars().take(MAX_NAME_CHARS).collect();
    }
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() { "cancion".into() } else { out }
}

/// `stem.ext`, añadiendo " (2)", " (3)"… si el nombre ya está ocupado.
fn unique_name(stem: &str, ext: &str, taken: &HashSet<String>) -> String {
    let with_ext = |s: &str| if ext.is_empty() { s.to_string() } else { format!("{s}.{ext}") };
    let mut name = with_ext(stem);
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = with_ext(&format!("{stem} ({n})"));
        n += 1;
    }
    name
}

/// Nombres que ya están en la carpeta (en minúsculas: Windows no distingue mayúsculas).
fn names_in(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Descarga las canciones que falten. Devuelve un resumen para la interfaz.
pub async fn download_playlist(config: &Config, playlist: &PlaylistResult, progress: Progress) -> Result<String> {
    let (bin, base, format) = (config.ytdlp_path.trim(), config.download_dir.as_str(), config.audio_format);
    if base.trim().is_empty() {
        bail!("Elige una carpeta de descargas en Ajustes.");
    }
    ytdlp::version(bin).await?;
    let ffmpeg = match ffmpeg::find(&config.ffmpeg_path).await {
        Ok(f) => Some(f.path),
        Err(e) if format.needs_ffmpeg() => return Err(e),
        Err(_) => {
            progress("Aviso: sin ffmpeg las canciones se guardan sin carátula ni etiquetas.".into());
            None
        }
    };
    progress(format!("Formato: {}", format.label()));
    let dir = playlist_dir(base, playlist);
    std::fs::create_dir_all(&dir).with_context(|| format!("no se pudo crear la carpeta {}", dir.display()))?;
    let mut manifest = Manifest::load(&dir);

    let total = playlist.items.len();
    let (mut done, mut skipped, mut failed) = (0, 0, Vec::new());
    for (i, item) in playlist.items.iter().enumerate() {
        if manifest.file(&dir, &item.video_id).is_some() {
            skipped += 1;
            continue;
        }
        progress(format!("Descargando {}/{total}: {}", i + 1, item.song));
        match ytdlp::download_audio(bin, &item.video_id, &dir, format, ffmpeg.as_deref()).await {
            Ok(path) => {
                // Nombre provisional legible; "Limpiar nombres" lo deja en "{cantante} - {canción}".
                let mut taken = names_in(&dir);
                taken.remove(&path.file_name().unwrap_or_default().to_string_lossy().to_lowercase());
                let name = unique_name(&sanitize_file_name(&item.video_title), &extension(&path), &taken);
                std::fs::rename(&path, dir.join(&name)).context("no se pudo renombrar la descarga")?;
                manifest.files.insert(item.video_id.clone(), name);
                manifest.save(&dir)?;
                done += 1;
            }
            Err(e) => {
                progress(format!("Aviso: no se pudo descargar \"{}\": {e:#}", item.song));
                failed.push(item.song.clone());
            }
        }
    }

    let mut summary = format!("Descarga terminada: {done} nuevas");
    if skipped > 0 {
        summary.push_str(&format!(", {skipped} ya estaban"));
    }
    if !failed.is_empty() {
        summary.push_str(&format!(", fallaron {}: {}", failed.len(), failed.join(", ")));
    }
    summary.push('.');
    progress(summary.clone());
    Ok(summary)
}

#[derive(Deserialize)]
struct CleanNames {
    names: Vec<CleanName>,
}

#[derive(Deserialize)]
struct CleanName {
    index: usize,
    name: String,
}

/// Renombra los archivos descargados a "{cantante} - {canción}" con ayuda del LLM.
pub async fn clean_names(llm: &LlmClient, playlist: &PlaylistResult, base: &str, progress: Progress) -> Result<String> {
    let dir = playlist_dir(base, playlist);
    let mut manifest = Manifest::load(&dir);
    let files: Vec<(&PlaylistItem, PathBuf)> = playlist
        .items
        .iter()
        .filter_map(|it| Some((it, manifest.file(&dir, &it.video_id)?)))
        .collect();
    if files.is_empty() {
        bail!("No hay canciones descargadas de esta playlist. Descárgalas primero.");
    }

    progress(format!("Pidiendo a {} nombres limpios para {} archivos…", llm.provider_label(), files.len()));
    let mut listing = String::new();
    for (i, (item, path)) in files.iter().enumerate() {
        listing.push_str(&format!(
            "{i}. archivo=\"{}\" | título del vídeo=\"{}\" | canción=\"{}\"\n",
            path.file_stem().unwrap_or_default().to_string_lossy(),
            item.video_title,
            item.song,
        ));
    }
    let schema = json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "index": {"type": "integer"},
                        "name": {"type": "string"}
                    },
                    "required": ["index", "name"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["names"],
        "additionalProperties": false
    });
    let system = "Eres un bibliotecario musical que ordena colecciones de archivos de audio.";
    let user = format!(
        "Artista de la playlist: {artist}\n\n\
         Para cada archivo, escribe un nombre limpio con el formato \"{{cantante}} - {{canción}}\", sin extensión.\n\
         - El cantante es el artista principal, bien escrito.\n\
         - Quita añadidos como (Official Audio), (Official Video), (Lyrics), HD, [id] o \"Remastered\".\n\
         - Conserva los artistas invitados al final: \"Canción (feat. Otro)\".\n\
         - Usa las mayúsculas del título oficial. No uses los caracteres < > : \" / \\ | ? *\n\n\
         {listing}\n\
         Responde en json: {{\"names\": [{{\"index\": 0, \"name\": \"{artist} - Canción\"}}]}}",
        artist = playlist.artist,
    );
    let value = llm.chat_json(system, &user, &schema).await?;
    let answer: CleanNames = serde_json::from_value(value).context("el LLM devolvió un formato inesperado")?;

    let mut taken = names_in(&dir);
    let mut renamed = 0;
    for (i, (item, path)) in files.iter().enumerate() {
        let proposed = answer
            .names
            .iter()
            .find(|n| n.index == i)
            .map(|n| n.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("{} - {}", playlist.artist, item.song));
        let current = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        taken.remove(&current.to_lowercase());
        let name = unique_name(&sanitize_file_name(&proposed), &extension(path), &taken);
        taken.insert(name.to_lowercase());
        if name == current {
            continue;
        }
        std::fs::rename(path, dir.join(&name)).with_context(|| format!("no se pudo renombrar \"{current}\""))?;
        manifest.files.insert(item.video_id.clone(), name);
        renamed += 1;
    }
    manifest.save(&dir)?;
    let summary = format!("Nombres limpios: {renamed} archivo(s) renombrado(s).");
    progress(summary.clone());
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limpia_caracteres_prohibidos() {
        assert_eq!(sanitize_file_name("AC/DC - Back In Black?"), "AC DC - Back In Black");
        assert_eq!(sanitize_file_name("  Canción:  final.  "), "Canción final");
        assert_eq!(sanitize_file_name("***"), "cancion");
    }

    #[test]
    fn evita_nombres_repetidos_sin_distinguir_mayusculas() {
        let taken: HashSet<String> = ["queen - song.m4a", "queen - song (2).m4a"].iter().map(|s| s.to_string()).collect();
        assert_eq!(unique_name("Queen - Song", "m4a", &taken), "Queen - Song (3).m4a");
        assert_eq!(unique_name("Queen - Otra", "m4a", &taken), "Queen - Otra.m4a");
    }

    /// Descarga real de una canción con yt-dlp: `cargo test -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn descarga_real_con_yt_dlp() {
        use crate::history::SourceKind;
        let base = std::env::temp_dir().join(format!("playlistyt-dl-{}", std::process::id()));
        let playlist = PlaylistResult {
            playlist_id: "test".into(),
            title: "Vicente Fernández — Top 1".into(),
            artist: "Vicente Fernández".into(),
            created_at: chrono::Local::now(),
            items: vec![PlaylistItem {
                song: "El Rey".into(),
                video_id: "5jmE8UsaM2U".into(),
                video_title: "El Rey".into(),
                channel: None,
                duration_secs: None,
                kind: SourceKind::AudioOriginal,
            }],
            missing: vec![],
        };
        let progress: Progress = std::sync::Arc::new(|m| println!("{m}"));
        for format in crate::config::AudioFormat::ALL {
            let config = Config {
                download_dir: base.join(format.extension()).to_string_lossy().into_owned(),
                audio_format: format,
                ..Config::default()
            };
            download_playlist(&config, &playlist, progress.clone()).await.unwrap();
            let dir = playlist_dir(&config.download_dir, &playlist);
            let file = dir.join(format!("El Rey.{}", format.extension()));
            assert!(file.is_file(), "falta {}", file.display());
            println!("{} -> {} bytes", file.display(), std::fs::metadata(&file).unwrap().len());
            assert_eq!(Manifest::load(&dir).downloaded(&dir, &playlist.items), 1);
            // La segunda vez no vuelve a descargar.
            let summary = download_playlist(&config, &playlist, progress.clone()).await.unwrap();
            assert!(summary.contains("1 ya estaban"), "{summary}");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn manifiesto_ignora_archivos_borrados() {
        let dir = std::env::temp_dir().join(format!("playlistyt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.m4a"), b"x").unwrap();
        let mut m = Manifest::default();
        m.files.insert("A".into(), "a.m4a".into());
        m.files.insert("B".into(), "b.m4a".into());
        m.save(&dir).unwrap();
        let m = Manifest::load(&dir);
        assert!(m.file(&dir, "A").is_some());
        assert!(m.file(&dir, "B").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

//! yt-dlp: búsqueda de vídeos (no gasta cuota de la YouTube Data API) y descarga de audio.

use crate::config::AudioFormat;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Sección "Canciones" de YouTube Music: pistas de audio de los sellos.
    Music,
    /// Búsqueda normal de YouTube.
    Video,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration_secs: Option<u32>,
    pub views: Option<u64>,
    pub source: Source,
    /// Posición en los resultados de su búsqueda (0 = primero).
    pub rank: usize,
}

#[derive(Deserialize)]
struct SearchJson {
    #[serde(default)]
    entries: Vec<EntryJson>,
}

#[derive(Deserialize)]
struct EntryJson {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    view_count: Option<u64>,
}

pub(crate) fn command(bin: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

async fn run(bin: &str, args: &[&str]) -> Result<String> {
    let output = command(bin).args(args).output().await.with_context(|| {
        format!("No se pudo ejecutar yt-dlp ('{bin}'). ¿Está instalado? Revisa la ruta en Ajustes.")
    })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        bail!("yt-dlp falló: {line}. Prueba a actualizar yt-dlp desde Ajustes.");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Busca `query` en YouTube Music (canciones) o en YouTube y devuelve hasta `max` resultados.
pub async fn search(bin: &str, query: &str, source: Source, max: usize) -> Result<Vec<Candidate>> {
    let max_s = max.to_string();
    let target = match source {
        Source::Music => format!(
            "https://music.youtube.com/search?q={}#songs",
            encode_query(query)
        ),
        Source::Video => format!("ytsearch{max}:{query}"),
    };
    let out = run(
        bin,
        &[
            &target,
            "--flat-playlist",
            "-J",
            "--no-warnings",
            "--playlist-end",
            &max_s,
        ],
    )
    .await?;
    parse_search_json(&out, source)
}

pub fn parse_search_json(json: &str, source: Source) -> Result<Vec<Candidate>> {
    let parsed: SearchJson =
        serde_json::from_str(json).context("yt-dlp devolvió un JSON inesperado")?;
    Ok(parsed
        .entries
        .into_iter()
        .filter_map(|e| Some((e.id?, e.title?, e.channel.or(e.uploader), e.duration, e.view_count)))
        .enumerate()
        .map(|(rank, (id, title, channel, duration, views))| Candidate {
            id,
            title,
            channel,
            duration_secs: duration.map(|d| d.round() as u32),
            views,
            source,
            rank,
        })
        .collect())
}

fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for b in q.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Descarga el audio de un vídeo en `dir` como `{id}.{ext}` y devuelve la ruta del archivo.
/// Con ffmpeg convierte al formato pedido e incrusta etiquetas y carátula; sin ffmpeg guarda
/// la mejor pista m4a (o la mejor que haya) tal cual.
pub async fn download_audio(
    bin: &str,
    video_id: &str,
    dir: &Path,
    format: AudioFormat,
    ffmpeg: Option<&Path>,
) -> Result<PathBuf> {
    let args = download_args(video_id, dir, format, ffmpeg)?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(bin, &args).await?;
    let expected = ffmpeg.map(|_| format.extension());
    std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            p.file_stem().is_some_and(|s| s == video_id)
                && match expected {
                    Some(want) => ext == want,
                    None => !["part", "ytdl", "jpg", "webp", "png"].contains(&ext.as_str()),
                }
        })
        .with_context(|| format!("yt-dlp terminó pero no aparece el archivo de {video_id}"))
}

fn download_args(video_id: &str, dir: &Path, format: AudioFormat, ffmpeg: Option<&Path>) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        format!("https://www.youtube.com/watch?v={video_id}"),
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--no-progress".into(),
        "-P".into(),
        dir.to_string_lossy().into_owned(),
        "-o".into(),
        "%(id)s.%(ext)s".into(),
        "-f".into(),
    ];
    match (format, ffmpeg) {
        (AudioFormat::M4a, None) => args.push("bestaudio[ext=m4a]/bestaudio".into()),
        (AudioFormat::Mp3, None) => bail!("Para descargar en MP3 hace falta ffmpeg (winget install Gyan.FFmpeg)."),
        (_, Some(ffmpeg)) => {
            args.push(if format == AudioFormat::M4a { "bestaudio[ext=m4a]/bestaudio" } else { "bestaudio" }.into());
            args.extend(["-x", "--audio-format", format.extension()].map(String::from));
            if format == AudioFormat::Mp3 {
                // VBR V0 (~245 kbps): transparente para fuentes de YouTube.
                args.extend(["--audio-quality", "0"].map(String::from));
            }
            args.extend(
                ["--embed-metadata", "--embed-thumbnail", "--convert-thumbnails", "jpg", "--ffmpeg-location"]
                    .map(String::from),
            );
            args.push(ffmpeg.to_string_lossy().into_owned());
        }
    }
    Ok(args)
}

pub async fn version(bin: &str) -> Result<String> {
    Ok(run(bin, &["--version"]).await?.trim().to_string())
}

/// `yt-dlp -U`. Solo funciona con el ejecutable oficial; si se instaló con winget o pip,
/// yt-dlp indica el comando correcto en su mensaje.
pub async fn update(bin: &str) -> Result<String> {
    let output = command(bin)
        .arg("-U")
        .output()
        .await
        .context("No se pudo ejecutar yt-dlp")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string();
    if output.status.success() {
        Ok(last)
    } else {
        bail!("{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/ytsearch_queen.json");

    #[test]
    fn parsea_busqueda_flat() {
        let c = parse_search_json(FIXTURE, Source::Video).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].id, "fJ9rUzIMcZQ");
        assert_eq!(c[0].channel.as_deref(), Some("Queen Official"));
        assert_eq!(c[0].duration_secs, Some(360));
        assert_eq!(c[2].rank, 2);
    }

    #[test]
    fn parsea_busqueda_music_sin_canal() {
        let json = r#"{"entries":[{"title":"Bohemian Rhapsody","id":"BSTsnWoslP4","_type":"url"}]}"#;
        let c = parse_search_json(json, Source::Music).unwrap();
        assert_eq!(c[0].channel, None);
        assert_eq!(c[0].source, Source::Music);
    }

    #[test]
    fn argumentos_de_descarga_segun_formato() {
        let dir = Path::new("C:\\musica");
        let sin = download_args("id1", dir, AudioFormat::M4a, None).unwrap();
        assert!(!sin.contains(&"-x".to_string()));
        assert!(download_args("id1", dir, AudioFormat::Mp3, None).is_err());

        let mp3 = download_args("id1", dir, AudioFormat::Mp3, Some(Path::new("C:\\ff\\ffmpeg.exe"))).unwrap();
        let joined = mp3.join(" ");
        assert!(joined.contains("-x --audio-format mp3 --audio-quality 0"), "{joined}");
        assert!(joined.contains("--embed-thumbnail"));
        assert!(joined.ends_with("--ffmpeg-location C:\\ff\\ffmpeg.exe"));
    }

    #[test]
    fn codifica_consulta() {
        assert_eq!(encode_query("Café Tacvba & co"), "Caf%C3%A9+Tacvba+%26+co");
    }
}

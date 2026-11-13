//! Localiza ffmpeg, que yt-dlp usa para convertir a MP3 y para incrustar carátula y etiquetas.

use crate::ytdlp::command;
use anyhow::{Result, bail};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub path: PathBuf,
    pub version: String,
}

/// Prueba la ruta configurada o, si está vacía, el PATH y la carpeta donde lo instala
/// `winget install Gyan.FFmpeg` (útil si la app se abrió antes de instalarlo).
pub async fn find(configured: &str) -> Result<Ffmpeg> {
    let configured = configured.trim();
    let candidates: Vec<PathBuf> = if configured.is_empty() {
        std::iter::once(PathBuf::from("ffmpeg")).chain(winget_candidates()).collect()
    } else {
        vec![PathBuf::from(configured)]
    };
    for path in candidates {
        let Ok(output) = command(&path.to_string_lossy()).arg("-version").output().await else { continue };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let version = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(2))
            .unwrap_or("?")
            .to_string();
        // yt-dlp no busca en el PATH lo que recibe en --ffmpeg-location: hay que darle la ruta completa.
        let path = crate::updates::which(&path.to_string_lossy()).unwrap_or(path);
        return Ok(Ffmpeg { path, version });
    }
    if configured.is_empty() {
        bail!("ffmpeg no está instalado. Instálalo con: winget install Gyan.FFmpeg");
    }
    bail!("No se pudo ejecutar ffmpeg en \"{configured}\".")
}

fn winget_candidates() -> Vec<PathBuf> {
    let Some(packages) = dirs::data_local_dir().map(|d| d.join("Microsoft").join("WinGet").join("Packages")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(packages) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Gyan.FFmpeg")))
        .filter_map(|pkg| std::fs::read_dir(pkg).ok())
        .flat_map(|builds| builds.filter_map(|e| e.ok().map(|e| e.path().join("bin").join("ffmpeg.exe"))))
        .filter(|exe| exe.is_file())
        .collect();
    // La versión más reciente primero ("ffmpeg-9.0.3-full_build" > "ffmpeg-9.0.2-full_build").
    found.sort();
    found.reverse();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// yt-dlp no busca en el PATH lo que recibe en --ffmpeg-location: necesita la ruta completa.
    #[tokio::test]
    async fn devuelve_la_ruta_completa() {
        let Ok(found) = find("").await else { return }; // sin ffmpeg instalado no hay nada que probar
        assert!(found.path.is_absolute(), "{}", found.path.display());
        assert!(found.path.is_file(), "{}", found.path.display());
    }
}

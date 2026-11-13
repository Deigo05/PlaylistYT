//! Actualizar yt-dlp y ffmpeg desde la app, con el mismo método con el que se instalaron
//! (pip, winget o el ejecutable oficial de yt-dlp).

use crate::ytdlp::{self, command};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// winget: no hay actualización disponible (APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE).
const WINGET_NO_UPDATE: i32 = 0x8A15_002B_u32 as i32;
/// winget: el paquete no está instalado (APPINSTALLER_CLI_ERROR_NO_APPLICATIONS_FOUND).
const WINGET_NOT_INSTALLED: i32 = 0x8A15_0014_u32 as i32;
const FFMPEG_PACKAGE: &str = "Gyan.FFmpeg";
const YTDLP_PACKAGE: &str = "yt-dlp.yt-dlp";

#[derive(Debug, PartialEq, Eq)]
enum Method {
    /// `python -m pip install -U yt-dlp` con el Python dueño de esa carpeta Scripts.
    Pip(PathBuf),
    Winget,
    /// `yt-dlp -U` (ejecutable oficial descargado a mano).
    SelfUpdate,
}

/// Carpeta donde winget deja los programas "portables" (yt-dlp, Deno…). winget la añade
/// al PATH del usuario, pero una app que ya estaba abierta no lo ve hasta reiniciarse.
fn winget_links() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Microsoft").join("WinGet").join("Links"))
}

/// Añade al final del PATH de este proceso la carpeta de winget, para encontrar lo que
/// se instale con la app abierta. Hay que llamarla al arrancar, antes de crear hilos.
pub fn add_winget_links_to_path() {
    let Some(links) = winget_links() else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let same = |p: &PathBuf| p.to_string_lossy().eq_ignore_ascii_case(&links.to_string_lossy());
    if paths.iter().any(same) {
        return;
    }
    paths.push(links);
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: se llama al principio de main, cuando todavía no hay otros hilos.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// Busca un ejecutable en el PATH como lo haría la consola.
fn which(bin: &str) -> Option<PathBuf> {
    let path = Path::new(bin);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let names = if path.extension().is_some() { vec![bin.to_string()] } else { vec![format!("{bin}.exe"), bin.to_string()] };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

fn method_for(exe: &Path) -> Method {
    let lower = exe.to_string_lossy().to_lowercase();
    if lower.contains("\\winget\\") {
        return Method::Winget;
    }
    if let Some(scripts) = exe.parent().filter(|d| d.file_name().is_some_and(|n| n.eq_ignore_ascii_case("scripts")))
        && let Some(python) = scripts.parent().map(|p| p.join("python.exe")).filter(|p| p.is_file())
    {
        return Method::Pip(python);
    }
    Method::SelfUpdate
}

/// Ejecuta un programa y devuelve (código de salida, texto de stdout + stderr).
async fn run(program: &Path, args: &[&str]) -> Result<(i32, String)> {
    let output = command(&program.to_string_lossy())
        .args(args)
        .output()
        .await
        .with_context(|| format!("no se pudo ejecutar {}", program.display()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Ok((output.status.code().unwrap_or(-1), text))
}

fn last_line(text: &str) -> String {
    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// `winget upgrade` del paquete; si no está instalado y `install_if_missing`, lo instala.
async fn winget(package: &str, install_if_missing: bool) -> Result<String> {
    let winget = PathBuf::from("winget");
    let flags = ["--id", package, "-e", "--silent", "--disable-interactivity", "--accept-source-agreements", "--accept-package-agreements"];
    let upgrade: Vec<&str> = std::iter::once("upgrade").chain(flags).collect();
    let (code, text) = run(&winget, &upgrade)
        .await
        .context("winget no está disponible en este equipo")?;
    match code {
        0 => Ok("actualizado a la última versión".into()),
        WINGET_NO_UPDATE => Ok("ya estaba en la última versión".into()),
        WINGET_NOT_INSTALLED if install_if_missing => {
            let install: Vec<&str> = std::iter::once("install").chain(flags).collect();
            let (code, text) = run(&winget, &install).await?;
            if code == 0 { Ok("instalado".into()) } else { bail!("winget no pudo instalarlo: {}", last_line(&text)) }
        }
        _ => bail!("winget no pudo actualizarlo: {}", last_line(&text)),
    }
}

/// Actualiza yt-dlp con el método con el que está instalado; si no está, lo instala con
/// winget (que trae también Deno y un ffmpeg). Devuelve un resumen con la versión.
pub async fn update_ytdlp(bin: &str) -> Result<String> {
    let bin = bin.trim();
    let Some(exe) = which(bin) else {
        if Path::new(bin).components().count() > 1 {
            bail!("no se encuentra yt-dlp en \"{bin}\". Corrige la ruta en Ajustes, o pon solo yt-dlp para instalarlo.");
        }
        winget(YTDLP_PACKAGE, true).await?;
        let exe = which(bin).context("yt-dlp se instaló, pero no se encuentra; cierra y vuelve a abrir la app")?;
        let version = ytdlp::version(&exe.to_string_lossy()).await?;
        return Ok(format!("yt-dlp instalado ({version})."));
    };
    let message = match method_for(&exe) {
        Method::Pip(python) => {
            let (code, text) = run(&python, &["-m", "pip", "install", "-U", "--disable-pip-version-check", "yt-dlp[default]"]).await?;
            if code != 0 {
                bail!("pip no pudo actualizar yt-dlp: {}", last_line(&text));
            }
            if text.contains("Successfully installed") {
                "actualizado con pip".to_string()
            } else {
                "ya estaba en la última versión".to_string()
            }
        }
        Method::Winget => winget(YTDLP_PACKAGE, false).await?,
        Method::SelfUpdate => ytdlp::update(&exe.to_string_lossy()).await?,
    };
    let version = ytdlp::version(&exe.to_string_lossy()).await?;
    Ok(format!("yt-dlp {message} ({version})."))
}

/// Actualiza ffmpeg con winget, o lo instala si no está. Si se configuró una ruta propia
/// fuera de winget, no se toca.
pub async fn update_ffmpeg(configured: &str) -> Result<String> {
    let configured = configured.trim();
    if !configured.is_empty() && !configured.to_lowercase().contains("\\winget\\") {
        bail!("ffmpeg está en una ruta propia ({configured}); actualízalo manualmente o deja la ruta vacía para usar el de winget.");
    }
    let message = winget(FFMPEG_PACKAGE, true).await?;
    Ok(format!("ffmpeg {message}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecta_el_metodo_de_instalacion() {
        let winget = Path::new(r"C:\Users\x\AppData\Local\Microsoft\WinGet\Links\yt-dlp.exe");
        assert_eq!(method_for(winget), Method::Winget);
        let suelto = Path::new(r"C:\herramientas\yt-dlp.exe");
        assert_eq!(method_for(suelto), Method::SelfUpdate);
    }

    /// Actualización real de yt-dlp y ffmpeg: `cargo test actualiza_de_verdad -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn actualiza_de_verdad() {
        println!("{:?}", which("yt-dlp").map(|p| method_for(&p)));
        println!("{:?}", update_ytdlp("yt-dlp").await);
        println!("{:?}", update_ffmpeg("").await);
    }

    #[tokio::test]
    async fn no_instala_si_la_ruta_propia_no_existe() {
        // Con una ruta propia que no existe no se lanza winget: se pide corregirla.
        let err = update_ytdlp(r"C:\no\existe\yt-dlp.exe").await.unwrap_err();
        assert!(err.to_string().contains("Corrige la ruta"), "{err}");
    }

    #[test]
    fn detecta_pip_si_hay_python_junto_a_scripts() {
        let root = std::env::temp_dir().join(format!("playlistyt-pip-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Scripts")).unwrap();
        std::fs::write(root.join("python.exe"), b"").unwrap();
        let exe = root.join("Scripts").join("yt-dlp.exe");
        assert_eq!(method_for(&exe), Method::Pip(root.join("python.exe")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}

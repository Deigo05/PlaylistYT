//! El agente: pide al LLM las canciones, busca cada una con yt-dlp, deja que el LLM elija
//! la mejor versión entre los candidatos filtrados y crea la playlist en YouTube.

use crate::config::Config;
use crate::history::{PlaylistItem, PlaylistResult};
use crate::llm::LlmClient;
use crate::quota::{Quota, playlist_cost};
use crate::ranking::{self, Scored, normalize};
use crate::youtube::{QuotaExceeded, YouTube};
use crate::ytdlp::{self, Source};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Canciones extra que se piden al LLM por si alguna no aparece en YouTube.
const RESERVE_SONGS: u32 = 3;
const CANDIDATES_FOR_LLM: usize = 5;
const PARALLEL_SEARCHES: usize = 3;
pub const MAX_SONGS: u32 = 50;

pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

pub struct Request {
    pub artist: String,
    pub count: u32,
    pub config: Config,
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
struct SongList {
    artist: String,
    songs: Vec<Song>,
}

#[derive(Debug, Clone, Deserialize)]
struct Song {
    title: String,
}

#[derive(Debug, Deserialize)]
struct Picks {
    picks: Vec<Pick>,
}

#[derive(Debug, Deserialize)]
struct Pick {
    song_index: usize,
    video_id: String,
}

struct SongSearch {
    song: Song,
    ranked: Vec<Scored>,
}

pub async fn create_playlist(
    req: Request,
    quota: Arc<Mutex<Quota>>,
    progress: Progress,
) -> Result<PlaylistResult> {
    let artist = req.artist.trim().to_string();
    if artist.is_empty() {
        bail!("Escribe el nombre de un cantante.");
    }
    if !(1..=MAX_SONGS).contains(&req.count) {
        bail!("El número de canciones debe estar entre 1 y {MAX_SONGS}.");
    }
    let cost = playlist_cost(req.count);
    let remaining = quota.lock().unwrap().remaining();
    if remaining < cost {
        bail!("Crear esta playlist cuesta {cost} unidades y solo quedan {remaining} hoy.");
    }
    let llm = LlmClient::new(req.config.llm.clone(), req.api_key.clone())?;
    let bin = req.config.ytdlp_path.trim().to_string();
    ytdlp::version(&bin).await?;

    // 1. Lista de canciones
    progress(format!(
        "Pidiendo a {} las {} canciones más populares de {artist}…",
        llm.provider_label(),
        req.count
    ));
    let list = ask_songs(&llm, &artist, req.count + RESERVE_SONGS).await?;
    let artist = if list.artist.trim().is_empty() { artist } else { list.artist.trim().to_string() };
    let mut songs = dedupe(list.songs);
    if songs.is_empty() {
        bail!("{} no reconoce a \"{artist}\". Revisa el nombre del cantante.", llm.provider_label());
    }
    let reserves = songs.split_off((req.count as usize).min(songs.len()));
    progress(format!("Lista de {} canciones: {}", songs.len(), titles(&songs)));

    // 2. Conectar con YouTube antes del trabajo largo (puede abrir el navegador)
    progress("Conectando con YouTube…".into());
    let youtube = YouTube::connect(&req.config, quota.clone()).await?;

    // 3. Buscar y filtrar candidatos
    let searches = search_all(&bin, &artist, &songs, &progress).await;

    // 4. Elegir la mejor versión de cada canción
    progress(format!("Eligiendo la versión original de cada canción con {}…", llm.provider_label()));
    let mut chosen: Vec<Option<PlaylistItem>> = match choose_with_llm(&llm, &artist, &searches).await {
        Ok(picks) => picks,
        Err(e) => {
            progress(format!("Aviso: el LLM no pudo elegir ({e:#}); uso la heurística."));
            searches.iter().map(|s| best_by_heuristic(s, &HashSet::new())).collect()
        }
    };

    // Sin duplicados
    let mut used = HashSet::new();
    for slot in chosen.iter_mut() {
        if let Some(item) = slot
            && !used.insert(item.video_id.clone())
        {
            *slot = None;
        }
    }

    // 5. Cubrir huecos con las canciones de reserva
    let mut missing: Vec<String> = Vec::new();
    let gaps = chosen.iter().filter(|c| c.is_none()).count();
    let mut items: Vec<PlaylistItem> = Vec::new();
    for (search, slot) in searches.iter().zip(chosen) {
        match slot {
            Some(item) => items.push(item),
            None => missing.push(search.song.title.clone()),
        }
    }
    if gaps > 0 && !reserves.is_empty() {
        progress(format!("Buscando {gaps} canción(es) de reserva…"));
        let extra = search_all(&bin, &artist, &reserves, &progress).await;
        for search in &extra {
            if items.len() >= req.count as usize {
                break;
            }
            if let Some(item) = best_by_heuristic(search, &used) {
                used.insert(item.video_id.clone());
                items.push(item);
            }
        }
    }
    if items.is_empty() {
        bail!("No se encontró ninguna canción de {artist} en YouTube.");
    }

    // 6. Crear la playlist
    let title = format!("{artist} — Top {}", items.len());
    progress(format!("Creando la playlist \"{title}\"…"));
    let playlist_id = youtube
        .create_playlist(
            &title,
            &format!(
                "Las {} canciones más populares de {artist}. Creada con PlaylistYT ({}).",
                items.len(),
                llm.provider_label()
            ),
        )
        .await?;

    let total = items.len();
    let mut added = Vec::with_capacity(total);
    let mut pending = items.into_iter().enumerate();
    while let Some((i, item)) = pending.next() {
        progress(format!("Añadiendo {}/{total}: {}", i + 1, item.song));
        match youtube.add_video(&playlist_id, &item.video_id).await {
            Ok(()) => added.push(item),
            Err(e) if e.is::<QuotaExceeded>() => {
                progress(format!("Aviso: {e}"));
                missing.push(item.song);
                missing.extend(pending.by_ref().map(|(_, it)| it.song));
            }
            Err(e) => {
                progress(format!("Aviso: no se pudo añadir \"{}\": {e:#}", item.song));
                missing.push(item.song);
            }
        }
    }

    progress(format!("Playlist creada con {} canciones.", added.len()));
    Ok(PlaylistResult {
        playlist_id,
        title,
        artist,
        created_at: chrono::Local::now(),
        items: added,
        missing,
    })
}

async fn ask_songs(llm: &LlmClient, artist: &str, n: u32) -> Result<SongList> {
    let schema = json!({
        "type": "object",
        "properties": {
            "artist": {"type": "string", "description": "Nombre canónico del artista"},
            "songs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "year": {"type": "integer"}
                    },
                    "required": ["title", "year"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["artist", "songs"],
        "additionalProperties": false
    });
    let system = "Eres un experto en música popular que conoce los éxitos de cada artista y \
                  cuáles son sus canciones más escuchadas en plataformas como YouTube y Spotify.";
    let user = format!(
        "Artista: {artist}\n\n\
         Dame las {n} canciones más populares de este artista, ordenadas de más a menos popular.\n\
         - Solo canciones en las que el artista es el intérprete principal.\n\
         - Usa el título original de la versión de estudio, sin añadidos como \"(Live)\", \"(Remix)\" o \"(Remastered)\".\n\
         - Si no reconoces al artista, devuelve \"songs\" vacío. No inventes canciones.\n\
         - En \"artist\" pon el nombre del artista bien escrito. En \"year\" el año de lanzamiento (0 si no lo sabes).\n\n\
         Responde en json, por ejemplo: {{\"artist\": \"Queen\", \"songs\": [{{\"title\": \"Bohemian Rhapsody\", \"year\": 1975}}]}}"
    );
    let mut last_err = None;
    for _ in 0..2 {
        match llm.chat_json(system, &user, &schema).await {
            Ok(value) => match serde_json::from_value::<SongList>(value) {
                Ok(list) => return Ok(list),
                Err(e) => last_err = Some(anyhow::anyhow!("formato de respuesta inesperado: {e}")),
            },
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap())
}

fn dedupe(songs: Vec<Song>) -> Vec<Song> {
    let mut seen = HashSet::new();
    songs
        .into_iter()
        .filter(|s| !s.title.trim().is_empty() && seen.insert(normalize(&s.title)))
        .collect()
}

fn titles(songs: &[Song]) -> String {
    songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>().join(", ")
}

async fn search_all(bin: &str, artist: &str, songs: &[Song], progress: &Progress) -> Vec<SongSearch> {
    let semaphore = Arc::new(Semaphore::new(PARALLEL_SEARCHES));
    let total = songs.len();
    let tasks: Vec<_> = songs
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, song)| {
            let semaphore = semaphore.clone();
            let bin = bin.to_string();
            let artist = artist.to_string();
            let progress = progress.clone();
            tokio::spawn(async move {
                let _permit = semaphore.acquire_owned().await.expect("semáforo abierto");
                progress(format!("Buscando {}/{total}: {}", i + 1, song.title));
                let query = format!("{artist} {}", song.title);
                let (music, video) = tokio::join!(
                    ytdlp::search(&bin, &query, Source::Music, 4),
                    ytdlp::search(&bin, &query, Source::Video, 6),
                );
                let mut all = Vec::new();
                for result in [music, video] {
                    match result {
                        Ok(found) => all.extend(found),
                        Err(e) => progress(format!("Aviso: {}: {e:#}", song.title)),
                    }
                }
                let ranked = ranking::rank(&all, &song.title, &artist);
                SongSearch { song, ranked }
            })
        })
        .collect();

    let mut out = Vec::with_capacity(total);
    for (task, song) in tasks.into_iter().zip(songs) {
        out.push(task.await.unwrap_or_else(|_| SongSearch {
            song: song.clone(),
            ranked: Vec::new(),
        }));
    }
    out
}

fn to_item(song: &Song, s: &Scored) -> PlaylistItem {
    PlaylistItem {
        song: song.title.clone(),
        video_id: s.candidate.id.clone(),
        video_title: s.candidate.title.clone(),
        channel: s.candidate.channel.clone(),
        duration_secs: s.candidate.duration_secs,
        kind: s.kind,
    }
}

fn best_by_heuristic(search: &SongSearch, used: &HashSet<String>) -> Option<PlaylistItem> {
    search
        .ranked
        .iter()
        .find(|s| s.score > 0 && !used.contains(&s.candidate.id))
        .map(|s| to_item(&search.song, s))
}

async fn choose_with_llm(
    llm: &LlmClient,
    artist: &str,
    searches: &[SongSearch],
) -> Result<Vec<Option<PlaylistItem>>> {
    let mut listing = String::new();
    for (i, s) in searches.iter().enumerate() {
        listing.push_str(&format!("\nCanción {i}: \"{}\"\n", s.song.title));
        if s.ranked.is_empty() {
            listing.push_str("  (sin candidatos)\n");
        }
        for c in s.ranked.iter().take(CANDIDATES_FOR_LLM) {
            let source = match c.candidate.source {
                Source::Music => "YouTube Music (sección canciones = pista de audio)",
                Source::Video => "YouTube",
            };
            listing.push_str(&format!(
                "  - video_id={} | título=\"{}\" | canal={} | duración={} | vistas={} | fuente={} | tipo={} | puntuación={}\n",
                c.candidate.id,
                c.candidate.title,
                c.candidate.channel.as_deref().unwrap_or("desconocido"),
                c.candidate.duration_secs.map(format_duration).unwrap_or_else(|| "?".into()),
                c.candidate.views.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                source,
                c.kind.label(),
                c.score,
            ));
        }
    }

    let schema = json!({
        "type": "object",
        "properties": {
            "picks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "song_index": {"type": "integer"},
                        "video_id": {"type": "string"},
                        "reason": {"type": "string"}
                    },
                    "required": ["song_index", "video_id", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["picks"],
        "additionalProperties": false
    });
    let system = "Eres un curador musical. Eliges, entre resultados de búsqueda de YouTube, la versión \
                  original de estudio de cada canción, prefiriendo el audio a los videoclips.";
    let user = format!(
        "Artista: {artist}\n\
         Para cada canción elige UN video_id de su lista de candidatos, con estas prioridades:\n\
         1. Versión original de estudio del propio artista (nunca directos, covers, remixes, karaoke ni versiones aceleradas).\n\
         2. Preferir la pista de audio (YouTube Music o canal \"Artista - Topic\") o un \"Official Audio\" antes que el videoclip.\n\
         3. Si solo hay videoclip oficial, elegirlo. Si ningún candidato es la canción correcta, deja video_id vacío (\"\").\n\
         La puntuación es una heurística orientativa; usa tu criterio.\n\
         {listing}\n\
         Responde en json: {{\"picks\": [{{\"song_index\": 0, \"video_id\": \"...\", \"reason\": \"...\"}}]}}"
    );
    let value = llm.chat_json(system, &user, &schema).await?;
    let picks: Picks = serde_json::from_value(value)?;
    Ok(apply_picks(searches, &picks))
}

/// Aplica las elecciones del LLM aceptando solo ids que estaban entre los candidatos.
/// Si el LLM no eligió nada válido para una canción, se usa la heurística.
fn apply_picks(searches: &[SongSearch], picks: &Picks) -> Vec<Option<PlaylistItem>> {
    searches
        .iter()
        .enumerate()
        .map(|(i, search)| {
            let pick = picks.picks.iter().find(|p| p.song_index == i);
            match pick {
                Some(p) if p.video_id.trim().is_empty() => None,
                Some(p) => search
                    .ranked
                    .iter()
                    .find(|s| s.candidate.id == p.video_id.trim())
                    .map(|s| to_item(&search.song, s))
                    .or_else(|| best_by_heuristic(search, &HashSet::new())),
                None => best_by_heuristic(search, &HashSet::new()),
            }
        })
        .collect()
}

pub fn format_duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::Candidate;

    fn search(title: &str, ids: &[&str]) -> SongSearch {
        let candidates: Vec<Candidate> = ids
            .iter()
            .enumerate()
            .map(|(rank, id)| Candidate {
                id: id.to_string(),
                title: title.into(),
                channel: None,
                duration_secs: Some(200),
                views: None,
                source: Source::Music,
                rank,
            })
            .collect();
        SongSearch {
            song: Song { title: title.into() },
            ranked: ranking::rank(&candidates, title, "Artista"),
        }
    }

    fn picks(list: &[(usize, &str)]) -> Picks {
        Picks {
            picks: list
                .iter()
                .map(|(i, id)| Pick { song_index: *i, video_id: id.to_string() })
                .collect(),
        }
    }

    #[test]
    fn acepta_ids_validos_del_llm() {
        let s = vec![search("Uno", &["a1", "a2"])];
        let r = apply_picks(&s, &picks(&[(0, "a2")]));
        assert_eq!(r[0].as_ref().unwrap().video_id, "a2");
    }

    #[test]
    fn rechaza_ids_inventados_y_usa_heuristica() {
        let s = vec![search("Uno", &["a1", "a2"])];
        let r = apply_picks(&s, &picks(&[(0, "inventado")]));
        assert_eq!(r[0].as_ref().unwrap().video_id, "a1");
    }

    #[test]
    fn id_vacio_deja_hueco_y_falta_de_pick_usa_heuristica() {
        let s = vec![search("Uno", &["a1"]), search("Dos", &["b1"])];
        let r = apply_picks(&s, &picks(&[(0, "")]));
        assert!(r[0].is_none());
        assert_eq!(r[1].as_ref().unwrap().video_id, "b1");
    }

    /// Búsqueda real con yt-dlp (sin LLM ni YouTube API): `cargo test -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn busqueda_real_con_yt_dlp() {
        let songs: Vec<Song> = ["Bohemian Rhapsody", "Don't Stop Me Now", "Under Pressure"]
            .iter()
            .map(|t| Song { title: t.to_string() })
            .collect();
        let progress: Progress = Arc::new(|m| println!("{m}"));
        let results = search_all("yt-dlp", "Queen", &songs, &progress).await;
        for s in &results {
            let best = best_by_heuristic(s, &HashSet::new()).expect("hay candidato");
            println!("{} -> {} [{}] {:?}", s.song.title, best.video_title, best.kind.label(), best.channel);
            for c in s.ranked.iter().take(5) {
                println!("    {:>4}  {} | {:?}", c.score, c.candidate.title, c.candidate.channel);
            }
        }
    }

    #[test]
    fn quita_canciones_repetidas() {
        let songs = vec![
            Song { title: "Bohemian Rhapsody".into() },
            Song { title: "bohemian rhapsody".into() },
            Song { title: " ".into() },
        ];
        assert_eq!(dedupe(songs).len(), 1);
    }
}

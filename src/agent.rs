//! El agente: pide al LLM las canciones, busca cada una con yt-dlp, deja que el LLM elija
//! la mejor versión entre los candidatos filtrados y crea la playlist en YouTube.

use crate::config::Config;
use crate::history::{PlaylistItem, PlaylistResult};
use crate::llm::LlmClient;
use crate::quota::{Quota, playlist_cost};
use crate::ranking::{self, Scored, normalize};
use crate::youtube::{QuotaExceeded, YouTube};
use crate::ytdlp::{self, Candidate, Source};
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
    /// Nombre limpio "Cantante - Canción" según el vídeo elegido.
    #[serde(default)]
    name: String,
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
    if items.len() < req.count as usize {
        progress(format!("Completando con las canciones más escuchadas de {artist} en YouTube Music…"));
        let max = (req.count as usize + 15).min(MAX_SONGS as usize);
        match ytdlp::search(&bin, &artist, Source::Music, max).await {
            Ok(top) => {
                let added = fill_with_top_songs(&top, &artist, req.count as usize, &mut items, &mut used);
                if added > 0 && !missing.is_empty() {
                    progress(format!(
                        "Aviso: no están en YouTube ({}); las sustituyo por otras {added} populares.",
                        missing.join(", ")
                    ));
                }
            }
            Err(e) => progress(format!("Aviso: {e:#}")),
        }
    }
    if items.is_empty() {
        bail!("No se encontró ninguna canción de {artist} en YouTube.");
    }
    // Las que no tienen nombre del LLM (reservas, heurística) usan "Cantante - Canción".
    for item in items.iter_mut().filter(|it| it.file_name.trim().is_empty()) {
        item.file_name = format!("{artist} - {}", item.song);
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
        file_name: String::new(),
    }
}

fn best_by_heuristic(search: &SongSearch, used: &HashSet<String>) -> Option<PlaylistItem> {
    search
        .ranked
        .iter()
        .find(|s| s.matches && s.score > 0 && !used.contains(&s.candidate.id))
        .map(|s| to_item(&search.song, s))
}

/// Completa la lista con las canciones que da YouTube Music al buscar solo el nombre del
/// artista (salen por popularidad). Hace falta cuando el LLM conoce poco al artista e
/// inventa títulos. Devuelve cuántas añadió.
fn fill_with_top_songs(
    top: &[Candidate],
    artist: &str,
    count: usize,
    items: &mut Vec<PlaylistItem>,
    used: &mut HashSet<String>,
) -> usize {
    let mut titles: HashSet<String> =
        items.iter().flat_map(|it| [normalize(&it.song), normalize(&it.video_title)]).collect();
    let before = items.len();
    for c in top {
        if items.len() >= count {
            break;
        }
        if used.contains(&c.id) || ranking::is_excluded(c, "") || !titles.insert(normalize(&c.title)) {
            continue;
        }
        used.insert(c.id.clone());
        items.push(PlaylistItem {
            song: c.title.clone(),
            video_id: c.id.clone(),
            video_title: c.title.clone(),
            channel: c.channel.clone(),
            duration_secs: c.duration_secs,
            kind: ranking::classify(c, artist),
            file_name: format!("{artist} - {}", c.title),
        });
    }
    items.len() - before
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
                        "name": {"type": "string"},
                        "reason": {"type": "string"}
                    },
                    "required": ["song_index", "video_id", "name", "reason"],
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
         En \"name\" escribe el nombre limpio de la canción elegida con el formato \"{{cantante}} - {{canción}}\": \
         artista principal bien escrito, título oficial sin añadidos como (Official Audio), (Lyrics) o Remastered, \
         y los invitados al final como \"(feat. Otro)\". Sin los caracteres < > : \" / \\ | ? *\n\
         {listing}\n\
         Responde en json: {{\"picks\": [{{\"song_index\": 0, \"video_id\": \"...\", \"name\": \"{artist} - Canción\", \"reason\": \"...\"}}]}}"
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
                Some(p) => match search.ranked.iter().find(|s| s.candidate.id == p.video_id.trim()) {
                    Some(s) => Some(PlaylistItem {
                        file_name: p.name.trim().to_string(),
                        ..to_item(&search.song, s)
                    }),
                    None => best_by_heuristic(search, &HashSet::new()),
                },
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
                .map(|(i, id)| Pick {
                    song_index: *i,
                    video_id: id.to_string(),
                    name: format!("Artista - Canción {i}"),
                })
                .collect(),
        }
    }

    #[test]
    fn acepta_ids_validos_del_llm() {
        let s = vec![search("Uno", &["a1", "a2"])];
        let r = apply_picks(&s, &picks(&[(0, "a2")]));
        assert_eq!(r[0].as_ref().unwrap().video_id, "a2");
        assert_eq!(r[0].as_ref().unwrap().file_name, "Artista - Canción 0");
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

    fn music(id: &str, title: &str, rank: usize) -> Candidate {
        Candidate {
            id: id.into(),
            title: title.into(),
            channel: None,
            duration_secs: None,
            views: None,
            source: Source::Music,
            rank,
        }
    }

    #[test]
    fn la_heuristica_no_acepta_otra_cancion() {
        // Si la canción no existe, YouTube Music devuelve otras del artista.
        let found = vec![music("p", "Passionfruit", 0), music("d", "Dark Beach", 1)];
        let s = SongSearch {
            song: Song { title: "Distant".into() },
            ranked: ranking::rank(&found, "Distant", "PastelGhost"),
        };
        assert!(best_by_heuristic(&s, &HashSet::new()).is_none());
    }

    #[test]
    fn completa_con_las_canciones_populares_del_artista() {
        let mut items = vec![to_item(
            &Song { title: "Dark Beach".into() },
            &ranking::rank(&[music("d", "Dark Beach", 0)], "Dark Beach", "PastelGhost")[0],
        )];
        let mut used: HashSet<String> = ["d".to_string()].into();
        let top = vec![
            music("d", "Dark Beach", 0),       // ya está (mismo id)
            music("d2", "Dark Beach", 1),      // ya está (mismo título)
            music("l", "Iris (Live)", 2),      // directo
            music("i", "Iris", 3),
            music("s", "Silhouette", 4),
            music("p", "Prism", 5),
        ];
        let added = fill_with_top_songs(&top, "PastelGhost", 3, &mut items, &mut used);
        assert_eq!(added, 2);
        let ids: Vec<_> = items.iter().map(|it| it.video_id.as_str()).collect();
        assert_eq!(ids, ["d", "i", "s"]);
        assert_eq!(items[1].file_name, "PastelGhost - Iris");
        assert!(used.contains("s"));
    }

    /// Artista poco conocido con títulos inventados (como los que dio el LLM para PastelGhost):
    /// no deben colarse otras canciones y los huecos se cubren con las más escuchadas.
    /// `cargo test completa_artista_poco_conocido -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn completa_artista_poco_conocido_de_verdad() {
        let artist = "PastelGhost";
        let songs: Vec<Song> = ["Dark Beach", "Distant", "Not Your Kind"]
            .iter()
            .map(|t| Song { title: t.to_string() })
            .collect();
        let progress: Progress = Arc::new(|m| println!("{m}"));
        let searches = search_all("yt-dlp", artist, &songs, &progress).await;
        let mut used = HashSet::new();
        let mut items: Vec<PlaylistItem> = searches.iter().filter_map(|s| best_by_heuristic(s, &used)).collect();
        assert_eq!(items.iter().map(|it| it.video_title.as_str()).collect::<Vec<_>>(), ["Dark Beach"]);
        used.extend(items.iter().map(|it| it.video_id.clone()));
        let top = ytdlp::search("yt-dlp", artist, Source::Music, 15).await.unwrap();
        fill_with_top_songs(&top, artist, 5, &mut items, &mut used);
        for it in &items {
            println!("{} [{}]", it.file_name, it.video_id);
        }
        assert_eq!(items.len(), 5);
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

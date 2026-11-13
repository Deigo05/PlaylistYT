//! Heurística para quedarse con la versión original y, a ser posible, con el audio.

use crate::history::SourceKind;
use crate::ytdlp::{Candidate, Source};

/// Palabras que indican que no es la versión de estudio original.
const EXCLUDED: &[&str] = &[
    "live",
    "en vivo",
    "ao vivo",
    "en directo",
    "concert",
    "concierto",
    "cover",
    "karaoke",
    "remix",
    "sped up",
    "slowed",
    "reverb",
    "8d",
    "nightcore",
    "instrumental",
    "acoustic",
    "acustico",
    "piano",
    "arrangement",
    "reaction",
    "reaccion",
    "flashmob",
    "flash mob",
    "parody",
    "parodia",
    "tutorial",
    "lesson",
    "bass boosted",
    "mashup",
    "demo",
    "rehearsal",
];

const MIN_SECS: u32 = 60;
const MAX_SECS: u32 = 15 * 60;

#[derive(Debug, Clone)]
pub struct Scored {
    pub candidate: Candidate,
    pub score: i32,
    pub kind: SourceKind,
    /// El título es el de la canción buscada (ver `title_matches`).
    pub matches: bool,
}

/// Minúsculas, sin acentos y con la puntuación convertida en espacios.
pub fn normalize(s: &str) -> String {
    let mapped: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ¿Contiene `phrase` como palabras completas? (ambos ya normalizados)
fn has_phrase(haystack: &str, phrase: &str) -> bool {
    format!(" {haystack} ").contains(&format!(" {phrase} "))
}

/// Título sin el texto entre paréntesis o corchetes: "Song (Remastered 2011)" → "Song".
fn strip_brackets(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Título sin paréntesis ni invitados ("feat. …"), normalizado.
fn core(title: &str) -> String {
    let t = normalize(&strip_brackets(title));
    [" feat ", " ft ", " featuring ", " with "]
        .iter()
        .fold(t, |t, sep| format!("{t} ").split(sep).next().unwrap_or_default().trim().to_string())
}

/// ¿El vídeo es esa canción? Su título, quitando paréntesis, invitados y el nombre del
/// artista delante o detrás, tiene que ser exactamente el de la canción. Así "Passionfruit"
/// o "Before You Fade" no pasan por "Distant" o "Fade".
pub fn title_matches(c: &Candidate, song: &str, artist: &str) -> bool {
    let song = core(song);
    let artist = normalize(artist);
    let mut title = core(&c.title);
    if !artist.is_empty() {
        if let Some(rest) = title.strip_prefix(&format!("{artist} ")) {
            title = rest.to_string();
        }
        if let Some(rest) = title.strip_suffix(&format!(" {artist}")) {
            title = rest.to_string();
        }
    }
    !song.is_empty() && title == song
}

pub fn is_excluded(c: &Candidate, song: &str) -> bool {
    if let Some(d) = c.duration_secs
        && !(MIN_SECS..=MAX_SECS).contains(&d)
    {
        return true;
    }
    let title = normalize(&c.title);
    let song = normalize(song);
    EXCLUDED
        .iter()
        .any(|w| has_phrase(&title, w) && !has_phrase(&song, w))
}

pub fn classify(c: &Candidate, artist: &str) -> SourceKind {
    let title = normalize(&c.title);
    let channel = c.channel.as_deref().map(normalize).unwrap_or_default();
    let artist = normalize(artist);
    if c.source == Source::Music || channel.ends_with(" topic") {
        SourceKind::AudioOriginal
    } else if has_phrase(&title, "official audio") || has_phrase(&title, "audio oficial") {
        SourceKind::AudioOficial
    } else if has_phrase(&title, "official video")
        || has_phrase(&title, "video oficial")
        || has_phrase(&title, "official music video")
        || has_phrase(&title, "videoclip")
        || channel.contains("vevo")
        || (!artist.is_empty() && channel.contains(&artist))
    {
        SourceKind::VideoOficial
    } else {
        SourceKind::Otro
    }
}

pub fn score(c: &Candidate, song: &str, artist: &str) -> i32 {
    let title = normalize(&c.title);
    let core_title = normalize(&strip_brackets(&c.title));
    let song_n = normalize(song);
    let channel = c.channel.as_deref().map(normalize).unwrap_or_default();
    let artist_n = normalize(artist);

    let mut s = match classify(c, artist) {
        SourceKind::AudioOriginal => 100,
        SourceKind::AudioOficial => 70,
        SourceKind::VideoOficial => 45,
        SourceKind::Otro => 20,
    };

    // ¿Es la canción que buscamos?
    if core_title == song_n || core_title == format!("{artist_n} {song_n}") {
        s += 30;
    } else if has_phrase(&title, &song_n) {
        s += 15;
    } else {
        s -= 60;
    }

    if !artist_n.is_empty() && (channel.contains(&artist_n) || channel.contains("vevo")) {
        s += 15;
    }
    if has_phrase(&title, "lyrics") || has_phrase(&title, "letra") || has_phrase(&title, "lyric video") {
        s -= 25;
    }
    // Los primeros resultados suelen ser los correctos.
    s - (c.rank as i32) * if c.source == Source::Music { 8 } else { 2 }
}

/// Filtra, puntúa y ordena (mejor primero) los candidatos de una canción.
pub fn rank(candidates: &[Candidate], song: &str, artist: &str) -> Vec<Scored> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Scored> = candidates
        .iter()
        .filter(|c| seen.insert(c.id.clone()))
        .filter(|c| !is_excluded(c, song))
        .map(|c| Scored {
            candidate: c.clone(),
            score: score(c, song, artist),
            kind: classify(c, artist),
            matches: title_matches(c, song, artist),
        })
        .collect();
    // Orden estable: con la misma puntuación se respeta el orden de búsqueda.
    out.sort_by_key(|s| std::cmp::Reverse(s.score));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, title: &str, channel: Option<&str>, secs: u32, source: Source, rank: usize) -> Candidate {
        Candidate {
            id: id.into(),
            title: title.into(),
            channel: channel.map(Into::into),
            duration_secs: Some(secs),
            views: None,
            source,
            rank,
        }
    }

    #[test]
    fn audio_de_youtube_music_gana_al_videoclip() {
        let c = vec![
            cand("video", "Queen – Bohemian Rhapsody (Official Video Remastered)", Some("Queen Official"), 360, Source::Video, 0),
            cand("lyrics", "Queen - Bohemian Rhapsody (Lyrics)", Some("7clouds Rock"), 355, Source::Video, 1),
            cand("music", "Bohemian Rhapsody", None, 355, Source::Music, 0),
        ];
        let r = rank(&c, "Bohemian Rhapsody", "Queen");
        assert_eq!(r[0].candidate.id, "music");
        assert_eq!(r[0].kind, SourceKind::AudioOriginal);
        assert_eq!(r[1].candidate.id, "video");
    }

    #[test]
    fn canal_topic_es_audio_original() {
        let c = cand("t", "Bohemian Rhapsody (Remastered 2011)", Some("Queen - Topic"), 355, Source::Video, 3);
        assert_eq!(classify(&c, "Queen"), SourceKind::AudioOriginal);
    }

    #[test]
    fn descarta_live_cover_y_duraciones_raras() {
        let song = "Bohemian Rhapsody";
        assert!(is_excluded(&cand("a", "Queen - Bohemian Rhapsody (Live Aid 1985)", None, 300, Source::Video, 0), song));
        assert!(is_excluded(&cand("b", "Bohemian Rhapsody cover", None, 300, Source::Video, 0), song));
        assert!(is_excluded(&cand("c", "Bohemian Rhapsody (Piano Arrangement)", None, 300, Source::Music, 0), song));
        assert!(is_excluded(&cand("d", "Bohemian Rhapsody", None, 30, Source::Video, 0), song));
        assert!(is_excluded(&cand("e", "Bohemian Rhapsody full album", None, 3600, Source::Video, 0), song));
        assert!(!is_excluded(&cand("f", "Queen - Bohemian Rhapsody (Official Video)", None, 360, Source::Video, 0), song));
    }

    #[test]
    fn no_descarta_si_la_cancion_contiene_la_palabra() {
        let c = cand("x", "Live and Let Die (Remastered 2018)", Some("Paul McCartney - Topic"), 190, Source::Video, 0);
        assert!(!is_excluded(&c, "Live and Let Die"));
        // "deliver" no es "live"
        let c = cand("y", "Deliverance", None, 200, Source::Video, 0);
        assert!(!is_excluded(&c, "Deliverance"));
    }

    #[test]
    fn otra_cancion_puntua_bajo() {
        let ok = cand("ok", "Bohemian Rhapsody", None, 355, Source::Music, 1);
        let other = cand("no", "The Show Must Go On", None, 263, Source::Music, 0);
        assert!(score(&ok, "Bohemian Rhapsody", "Queen") > score(&other, "Bohemian Rhapsody", "Queen"));
    }

    #[test]
    fn reconoce_si_el_titulo_es_la_cancion() {
        let t = |title: &str| cand("x", title, None, 200, Source::Video, 0);
        assert!(title_matches(&t("Fade"), "Fade", "PastelGhost"));
        assert!(title_matches(&t("PastelGhost - Fade (Official Audio)"), "Fade", "PastelGhost"));
        assert!(title_matches(&t("Fade - PastelGhost"), "Fade", "PastelGhost"));
        assert!(title_matches(&t("Queen – Bohemian Rhapsody (Remastered 2011)"), "Bohemian Rhapsody", "Queen"));
        assert!(title_matches(&t("Lovely (with Khalid)"), "lovely", "Billie Eilish"));
        // Otra canción que solo comparte una palabra, o ninguna.
        assert!(!title_matches(&t("Before You Fade"), "Fade", "PastelGhost"));
        assert!(!title_matches(&t("Passionfruit"), "Distant", "PastelGhost"));
    }

    #[test]
    fn normaliza_acentos_y_puntuacion() {
        assert_eq!(normalize("Café, Tacvba!  (En Vivo)"), "cafe tacvba en vivo");
    }
}

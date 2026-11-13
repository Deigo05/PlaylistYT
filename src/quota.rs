//! Contador local de la cuota diaria de la YouTube Data API.
//!
//! Google no expone la cuota restante en tiempo real, así que la app suma el coste
//! conocido de cada llamada. La cuota se reinicia a medianoche hora del Pacífico.

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use chrono_tz::America::Los_Angeles;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DAILY_LIMIT: u32 = 10_000;
pub const COST_INSERT: u32 = 50;
pub const COST_DELETE: u32 = 50;
pub const COST_LIST: u32 = 1;

/// Coste de crear una playlist con `songs` canciones (insert de la playlist + un insert por canción).
pub fn playlist_cost(songs: u32) -> u32 {
    COST_INSERT + COST_INSERT * songs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quota {
    day: NaiveDate,
    used: u32,
}

impl Quota {
    fn path() -> PathBuf {
        crate::config::data_dir().join("quota.json")
    }

    fn new(now: DateTime<Utc>) -> Self {
        Self {
            day: pacific_day(now),
            used: 0,
        }
    }

    pub fn load() -> Self {
        let mut q = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str::<Quota>(&s).ok())
            .unwrap_or_else(|| Quota::new(Utc::now()));
        q.refresh(Utc::now());
        q
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(Self::path(), text);
        }
    }

    /// Pone el contador a cero si ya cambió el día en hora del Pacífico.
    pub fn refresh(&mut self, now: DateTime<Utc>) {
        let today = pacific_day(now);
        if self.day != today {
            self.day = today;
            self.used = 0;
        }
    }

    pub fn add(&mut self, units: u32) {
        self.refresh(Utc::now());
        self.used = self.used.saturating_add(units);
        self.save();
    }

    /// YouTube respondió `quotaExceeded`: el dato real manda sobre el contador.
    pub fn exhaust(&mut self) {
        self.refresh(Utc::now());
        self.used = self.used.max(DAILY_LIMIT);
        self.save();
    }

    pub fn used(&self) -> u32 {
        self.used
    }

    pub fn remaining(&self) -> u32 {
        DAILY_LIMIT.saturating_sub(self.used)
    }
}

fn pacific_day(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&Los_Angeles).date_naive()
}

/// Próximo reinicio de la cuota, en hora local del usuario.
pub fn next_reset(now: DateTime<Utc>) -> DateTime<Local> {
    let tomorrow = pacific_day(now).succ_opt().expect("fecha válida");
    let midnight = tomorrow.and_hms_opt(0, 0, 0).expect("hora válida");
    Los_Angeles
        .from_local_datetime(&midnight)
        .earliest()
        .expect("medianoche PT existe")
        .with_timezone(&Local)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn se_reinicia_al_cruzar_medianoche_pacifico() {
        // 2026-11-12 07:59 UTC = 23:59 PST del día 11
        let mut q = Quota::new(utc("2026-11-12T07:59:00Z"));
        q.used = 4_000;
        q.refresh(utc("2026-11-12T07:59:30Z"));
        assert_eq!(q.used, 4_000);
        // 08:00 UTC = 00:00 PST del día 12
        q.refresh(utc("2026-11-12T08:00:00Z"));
        assert_eq!(q.used, 0);
        assert_eq!(q.remaining(), DAILY_LIMIT);
    }

    #[test]
    fn coste_de_playlist() {
        assert_eq!(playlist_cost(10), 550);
        assert_eq!(playlist_cost(5), 300);
    }

    #[test]
    fn proximo_reinicio_es_medianoche_pt() {
        let r = next_reset(utc("2026-11-12T20:00:00Z"));
        let pt = r.with_timezone(&Los_Angeles);
        assert_eq!(pt.format("%Y-%m-%d %H:%M").to_string(), "2026-11-13 00:00");
    }
}

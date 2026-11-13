//! Interfaz: formulario (cantante + nº de canciones), progreso, playlist creada,
//! historial y ventana de ajustes.

use crate::agent::{self, MAX_SONGS, Request};
use crate::config::{self, AudioFormat, Config, Provider};
use crate::{download, theme};
use crate::history::{self, PlaylistResult};
use crate::quota::{self, COST_DELETE, DAILY_LIMIT, Quota, playlist_cost};
use crate::worker::{Command, Event, Worker};
use chrono::Utc;
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::sync::{Arc, Mutex};

const MAX_PROGRESS_LINES: usize = 200;

struct SettingsDraft {
    config: Config,
    api_key: String,
}

enum SettingsAction {
    None,
    Save,
    Cancel,
}

pub struct PlaylistApp {
    config: Config,
    api_key: String,
    settings: Option<SettingsDraft>,
    artist: String,
    count: u32,
    running: bool,
    progress: Vec<String>,
    error: Option<String>,
    history: Vec<PlaylistResult>,
    selected: Option<usize>,
    quota: Arc<Mutex<Quota>>,
    worker: Worker,
    ytdlp_status: Option<Result<String, String>>,
    ytdlp_busy: bool,
    youtube_status: Option<Result<String, String>>,
    youtube_busy: bool,
    /// Playlist (id) pendiente de confirmar su borrado.
    confirm_delete: Option<String>,
    /// Playlist (id) que se está borrando.
    deleting: Option<String>,
    /// Descarga en curso: (id de la playlist, texto a mostrar).
    files_task: Option<(String, &'static str)>,
    /// Versión de ffmpeg o motivo por el que no está disponible.
    ffmpeg_status: Option<Result<String, String>>,
    ffmpeg_busy: bool,
    /// Se está instalando lo que faltaba desde el aviso de primer uso.
    tools_installing: bool,
}

enum FilesAction {
    Download,
    OpenFolder,
}

impl PlaylistApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let config = Config::load();
        let api_key = config::load_api_key(config.llm.provider);
        let quota = Arc::new(Mutex::new(Quota::load()));
        let worker = Worker::start(cc.egui_ctx.clone(), quota.clone());
        let history = history::load();

        worker.send(Command::CheckYtdlp(config.ytdlp_path.clone()));
        worker.send(Command::CheckFfmpeg(config.ffmpeg_path.clone()));
        let youtube_busy = Config::tokens_path().exists() && !config.client_secret_path.is_empty();
        if youtube_busy {
            worker.send(Command::ConnectYoutube(config.clone()));
        }

        Self {
            config,
            api_key,
            settings: None,
            artist: String::new(),
            count: 10,
            running: false,
            progress: Vec::new(),
            error: None,
            selected: if history.is_empty() { None } else { Some(0) },
            history,
            quota,
            worker,
            ytdlp_status: None,
            ytdlp_busy: true,
            youtube_status: None,
            youtube_busy,
            confirm_delete: None,
            deleting: None,
            files_task: None,
            ffmpeg_status: None,
            ffmpeg_busy: true,
            tools_installing: false,
        }
    }

    fn handle_events(&mut self) {
        for event in self.worker.poll() {
            match event {
                Event::Progress(msg) => {
                    self.progress.push(msg);
                    if self.progress.len() > MAX_PROGRESS_LINES {
                        self.progress.remove(0);
                    }
                }
                Event::Done(result) => {
                    self.running = false;
                    self.youtube_status.get_or_insert(Ok("conectado".into()));
                    self.history.insert(0, result);
                    history::save(&self.history);
                    self.selected = Some(0);
                }
                Event::Failed(msg) => {
                    self.running = false;
                    self.error = Some(msg);
                }
                Event::PlaylistDeleted { playlist_id, result } => {
                    self.deleting = None;
                    match result {
                        Ok(()) => self.remove_from_history(&playlist_id),
                        Err(msg) => self.error = Some(format!("No se pudo borrar la playlist: {msg}")),
                    }
                }
                Event::FfmpegFound(result) => {
                    self.ffmpeg_busy = false;
                    self.ffmpeg_status = Some(result);
                }
                Event::FilesTaskDone(result) => {
                    self.files_task = None;
                    if let Err(msg) = result {
                        self.error = Some(msg);
                    }
                }
                Event::YoutubeConnected(result) => {
                    self.youtube_busy = false;
                    self.youtube_status = Some(result);
                }
                Event::YtdlpVersion(result) => {
                    self.ytdlp_busy = false;
                    self.ytdlp_status = Some(result);
                }
                Event::YtdlpUpdated(result) => {
                    match result {
                        Ok(msg) => self.progress.push(msg),
                        Err(msg) => self.error = Some(format!("No se pudo instalar o actualizar yt-dlp: {msg}")),
                    }
                    self.worker.send(Command::CheckYtdlp(self.config.ytdlp_path.clone()));
                }
                Event::FfmpegUpdated(result) => {
                    match result {
                        Ok(msg) => self.progress.push(msg),
                        Err(msg) => self.error = Some(format!("No se pudo instalar o actualizar ffmpeg: {msg}")),
                    }
                    self.worker.send(Command::CheckFfmpeg(self.config.ffmpeg_path.clone()));
                }
            }
        }
    }

    fn start_creation(&mut self) {
        self.running = true;
        self.error = None;
        self.progress.clear();
        self.worker.send(Command::Create(Request {
            artist: self.artist.trim().to_string(),
            count: self.count,
            config: self.config.clone(),
            api_key: self.api_key.clone(),
        }));
    }

    fn remove_from_history(&mut self, playlist_id: &str) {
        let Some(pos) = self.history.iter().position(|p| p.playlist_id == playlist_id) else { return };
        let removed = self.history.remove(pos);
        history::save(&self.history);
        self.progress.push(format!("Playlist \"{}\" borrada de YouTube.", removed.title));
        self.selected = match self.selected {
            _ if self.history.is_empty() => None,
            Some(s) if s > pos => Some(s - 1),
            Some(s) => Some(s.min(self.history.len() - 1)),
            None => None,
        };
    }

    fn open_settings(&mut self) {
        if self.settings.is_none() {
            self.settings = Some(SettingsDraft {
                config: self.config.clone(),
                api_key: self.api_key.clone(),
            });
        }
    }

    /// Franja superior: nombre de la app, lo que se está viendo y el estado de YouTube.
    fn top_bar(&mut self, ui: &mut egui::Ui, playlist: Option<&PlaylistResult>) {
        ui.horizontal(|ui| {
            ui.label(theme::semibold("PlaylistYT", 13.5));
            ui.add_space(6.0);
            let context = playlist.map_or("nueva playlist", |p| p.title.as_str());
            ui.label(RichText::new(context).monospace().size(11.5).color(theme::MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &self.youtube_status {
                _ if self.youtube_busy => {
                    theme::pill(ui, theme::MUTED, "Conectando con YouTube…");
                }
                Some(Ok(channel)) => {
                    theme::pill(ui, theme::GREEN, channel).on_hover_text("Cuenta de YouTube conectada");
                }
                Some(Err(e)) => {
                    theme::pill(ui, theme::WARN, "YouTube sin conectar").on_hover_text(e);
                }
                None => {
                    theme::pill(ui, theme::WARN, "YouTube sin conectar").on_hover_text("Conéctalo en Ajustes");
                }
            });
        });
    }

    /// Barra lateral: nueva playlist, las ya creadas y los ajustes.
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        if theme::sidebar_item(ui, self.selected.is_none(), "+   Nueva playlist", None).clicked() {
            self.selected = None;
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(theme::meta("Playlists"));
        });
        ui.add_space(2.0);
        let list_height = (ui.available_height() - 46.0).max(40.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.history.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(8.0);
                        ui.label(theme::meta("Aún no hay ninguna. Las que crees aparecen aquí."));
                    });
                }
                for (i, p) in self.history.iter().enumerate() {
                    // Verde: están todas las canciones. Ámbar: alguna no se encontró.
                    let dot = if p.missing.is_empty() { theme::GREEN } else { theme::WARN };
                    let resp = theme::sidebar_item(ui, self.selected == Some(i), &p.title, Some(dot));
                    if resp.on_hover_text(format!("{} canciones · {}", p.items.len(), p.created_at.format("%d/%m/%Y"))).clicked() {
                        self.selected = Some(i);
                    }
                }
            });
        ui.add_space(4.0);
        ui.separator();
        if theme::sidebar_item(ui, self.settings.is_some(), "Ajustes", None).clicked() {
            self.open_settings();
        }
    }

    /// Aviso de primer uso: si falta yt-dlp o ffmpeg, ofrece instalarlos con winget.
    fn tools_banner(&mut self, ui: &mut egui::Ui) {
        let installing = self.tools_installing && (self.ytdlp_busy || self.ffmpeg_busy);
        let missing_ytdlp = !self.ytdlp_busy && matches!(self.ytdlp_status, Some(Err(_)));
        let missing_ffmpeg = !self.ffmpeg_busy && matches!(self.ffmpeg_status, Some(Err(_)));
        if !installing && !missing_ytdlp && !missing_ffmpeg {
            self.tools_installing = false;
            return;
        }
        let mut install = false;
        theme::alert_card(theme::WARN).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if installing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(theme::semibold("Instalando herramientas…", 14.0));
                });
                ui.label(theme::meta("winget las descarga e instala; tarda uno o dos minutos."));
                return;
            }
            let mut what = Vec::new();
            if missing_ytdlp {
                what.push("yt-dlp (para buscar y descargar)");
            }
            if missing_ffmpeg {
                what.push("ffmpeg (para MP3, carátula y etiquetas)");
            }
            ui.label(theme::semibold("Faltan herramientas", 14.0));
            ui.label(what.join("  ·  "));
            ui.label(theme::meta("Se instalan con winget, incluido en Windows 10 y 11."));
            ui.add_space(4.0);
            let busy = self.running || self.files_task.is_some();
            install = ui.add_enabled(!busy, theme::primary("Instalar lo que falta")).clicked();
        });
        ui.add_space(10.0);
        if install {
            self.tools_installing = true;
            self.error = None;
            self.ytdlp_busy |= missing_ytdlp;
            self.ffmpeg_busy |= missing_ffmpeg;
            self.worker.send(Command::InstallMissing {
                ytdlp: missing_ytdlp.then(|| self.config.ytdlp_path.clone()),
                ffmpeg: missing_ffmpeg.then(|| self.config.ffmpeg_path.clone()),
            });
        }
    }

    /// Pantalla "Nueva playlist": cantante, número de canciones y botón de crear.
    fn new_view(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(theme::semibold("¿Qué playlist quieres crear?", 22.0));
            ui.label(RichText::new("Escribe un cantante, elige cuántas canciones y pulsa Crear playlist.").color(theme::MUTED));
            ui.add_space(12.0);
            self.tools_banner(ui);
            self.form(ui);
            self.activity_card(ui);
        });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        let cost = playlist_cost(self.count);
        let remaining = self.quota.lock().unwrap().remaining();
        let blocker = if self.running {
            Some("Creando playlist…".to_string())
        } else if self.artist.trim().is_empty() {
            Some("Escribe el nombre del cantante.".to_string())
        } else if remaining < cost {
            Some(format!("Cuota insuficiente: quedan {} unidades.", thousands(remaining)))
        } else if self.config.llm.provider.needs_api_key() && self.api_key.trim().is_empty() {
            Some(format!("Falta la API key de {} (en Ajustes).", self.config.llm.provider.label()))
        } else if self.config.client_secret_path.trim().is_empty() {
            Some("Falta configurar YouTube (en Ajustes).".to_string())
        } else {
            None
        };

        let mut start = false;
        theme::card().inner_margin(egui::Margin::same(18)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section(ui, "1", "¿De qué cantante?");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.artist)
                    .hint_text("Ej.: Queen, Shakira, Bad Bunny")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(10, 8)),
            );
            start |= edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(16.0);
            theme::section(ui, "2", "¿Cuántas canciones?");
            ui.horizontal(|ui| {
                for n in [5, 10, 15, 20, 30, 50] {
                    if ui.add(theme::chip(self.count == n, &n.to_string())).clicked() {
                        self.count = n;
                    }
                }
                ui.add_space(8.0);
                ui.label(theme::meta("Otra cantidad"));
                ui.add(egui::DragValue::new(&mut self.count).range(1..=MAX_SONGS));
            });

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(theme::meta(format!(
                    "Coste: {} unidades de cuota  ·  IA: {}",
                    thousands(cost),
                    self.config.llm.provider.label()
                )));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    start |= ui.add_enabled(blocker.is_none(), theme::primary("Crear playlist")).clicked();
                    if self.running {
                        ui.spinner();
                    } else if let Some(reason) = &blocker {
                        ui.label(theme::meta(reason.as_str()));
                    }
                });
            });
        });
        if start && blocker.is_none() {
            self.start_creation();
        }
    }

    /// Tarjetas de error y de progreso (creación, descargas, instalaciones).
    fn activity_card(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            ui.add_space(10.0);
            theme::alert_card(theme::DANGER).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(theme::semibold("Algo ha fallado", 14.0));
                ui.label(RichText::new(err).color(theme::DANGER));
            });
        }
        if self.progress.is_empty() {
            return;
        }
        ui.add_space(10.0);
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(theme::meta("Progreso"));
            // Etapa de la creación según los mensajes del agente (si los hay).
            let stage = self.progress.iter().filter_map(|l| creation_stage(l)).max();
            if let Some(stage) = stage {
                ui.add_space(2.0);
                for (i, label) in ["Lista de canciones", "Buscar en YouTube", "Elegir la versión original", "Crear la playlist"]
                    .into_iter()
                    .enumerate()
                {
                    let state = if i < stage {
                        theme::Step::Done
                    } else if i == stage && self.running {
                        theme::Step::Active
                    } else {
                        theme::Step::Pending
                    };
                    theme::step(ui, state, label);
                }
                ui.add_space(4.0);
                ui.separator();
            }
            egui::ScrollArea::vertical()
                .id_salt("progress")
                .max_height(96.0)
                .auto_shrink([false, true])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let last = self.progress.len() - 1;
                    for (i, line) in self.progress.iter().enumerate() {
                        let color = if line.starts_with("Aviso") {
                            theme::WARN
                        } else if i == last {
                            theme::TEXT
                        } else {
                            theme::MUTED
                        };
                        ui.label(RichText::new(line).monospace().size(11.5).color(color));
                    }
                });
        });
    }

    /// Centro de la pantalla de una playlist: título, link, avisos y canciones.
    fn playlist_view(&mut self, ui: &mut egui::Ui, p: &PlaylistResult) {
        ui.add_space(6.0);
        ui.label(theme::semibold(&p.title, 22.0));
        let url = p.url();
        ui.hyperlink_to(RichText::new(&url).monospace().size(12.0), &url);
        if !p.missing.is_empty() {
            ui.add_space(8.0);
            theme::alert_card(theme::WARN).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(format!("No encontradas o no añadidas: {}", p.missing.join(", "))).color(theme::WARN));
            });
        }
        self.activity_card(ui);
        ui.add_space(10.0);

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            ui.label(theme::meta(format!("Canciones  ·  {}", p.items.len())));
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(26.0))
                .column(Column::initial(230.0).at_least(110.0).clip(true))
                .column(Column::initial(130.0).at_least(70.0).clip(true))
                .column(Column::exact(44.0))
                .column(Column::remainder().at_least(110.0))
                .header(24.0, |mut h| {
                    for title in ["#", "Canción", "Canal", "Dur.", "Tipo"] {
                        h.col(|ui| {
                            ui.label(theme::meta(title));
                        });
                    }
                })
                .body(|mut body| {
                    for (i, item) in p.items.iter().enumerate() {
                        body.row(30.0, |mut row| {
                            row.col(|ui| {
                                ui.label(theme::meta((i + 1).to_string()));
                            });
                            row.col(|ui| {
                                ui.hyperlink_to(RichText::new(&item.song).color(theme::TEXT), item.url())
                                    .on_hover_text(format!("Vídeo: {}", item.video_title));
                            });
                            row.col(|ui| {
                                ui.label(theme::meta(item.channel.as_deref().unwrap_or("YouTube Music")));
                            });
                            row.col(|ui| {
                                ui.label(theme::meta(item.duration_secs.map(agent::format_duration).unwrap_or_default()));
                            });
                            row.col(|ui| {
                                theme::kind_badge(ui, item.kind);
                            });
                        });
                    }
                });
        });
    }

    /// Panel derecho: datos y descarga de la playlist elegida, herramientas y cuota.
    fn info_panel(&mut self, ui: &mut egui::Ui, playlist: Option<&PlaylistResult>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if let Some(p) = playlist {
                self.playlist_card(ui, p);
                ui.add_space(6.0);
                self.download_card(ui, p);
                ui.add_space(6.0);
            }
            self.tools_card(ui);
            ui.add_space(6.0);
            self.quota_card(ui);
            if let Some(p) = playlist {
                ui.add_space(6.0);
                self.delete_button(ui, p);
            }
        });
    }

    fn playlist_card(&mut self, ui: &mut egui::Ui, p: &PlaylistResult) {
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(theme::semibold(&p.artist, 15.0));
            if p.missing.is_empty() {
                theme::badge(ui, "Completa", theme::GREEN);
            } else {
                theme::badge(ui, &format!("Faltan {}", p.missing.len()), theme::WARN);
            }
            ui.add_space(4.0);
            theme::row(ui, "Canciones", RichText::new(p.items.len().to_string()).color(theme::TEXT));
            theme::row(ui, "Creada", RichText::new(p.created_at.format("%d/%m/%Y %H:%M").to_string()).color(theme::TEXT));
            theme::row(ui, "Visibilidad", RichText::new("Oculta").color(theme::TEXT))
                .on_hover_text("Unlisted: solo la ve quien tenga el link");
            ui.add_space(4.0);
            if ui.add(theme::secondary("Copiar link")).clicked() {
                ui.ctx().copy_text(p.url());
            }
        });
    }

    fn download_card(&mut self, ui: &mut egui::Ui, p: &PlaylistResult) {
        let dir = download::playlist_dir(&self.config.download_dir, p);
        let downloaded = download::Manifest::load(&dir).downloaded(&dir, &p.items);
        let busy = self.running || self.files_task.is_some();
        let mut files_action = None;
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(theme::meta("Descarga"));
            theme::row(ui, "Descargadas", theme::semibold(format!("{downloaded} / {}", p.items.len()), 13.0));
            theme::progress_bar(ui, downloaded as f32 / p.items.len().max(1) as f32);
            ui.add_space(6.0);
            match &self.files_task {
                Some((id, label)) if *id == p.playlist_id => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(*label);
                    });
                }
                _ => {
                    let format = self.config.audio_format;
                    let has_ffmpeg = matches!(self.ffmpeg_status, Some(Ok(_)));
                    let button = theme::primary(&format!("Descargar en {}", format.extension().to_uppercase()))
                        .min_size(egui::vec2(ui.available_width(), 32.0));
                    let resp = ui.add_enabled(!busy && (has_ffmpeg || !format.needs_ffmpeg()), button);
                    let hover = if format.needs_ffmpeg() && !has_ffmpeg {
                        "MP3 necesita ffmpeg. Instálalo o cambia el formato en Ajustes.".to_string()
                    } else if has_ffmpeg {
                        format!("{}, con carátula y etiquetas. El formato se cambia en Ajustes.", format.label())
                    } else {
                        format!("{}. Sin ffmpeg no lleva carátula ni etiquetas.", format.label())
                    };
                    if resp.on_hover_text(&hover).on_disabled_hover_text(&hover).clicked() {
                        files_action = Some(FilesAction::Download);
                    }
                }
            }
            if ui.add(theme::secondary("Abrir carpeta").min_size(egui::vec2(ui.available_width(), 30.0))).clicked() {
                files_action = Some(FilesAction::OpenFolder);
            }
            ui.label(theme::meta("Descarga solo música sobre la que tengas derechos."));
        });
        match files_action {
            Some(FilesAction::Download) => {
                self.progress.clear();
                self.error = None;
                self.files_task = Some((p.playlist_id.clone(), "Descargando…"));
                self.worker.send(Command::Download(self.config.clone(), p.clone()));
            }
            Some(FilesAction::OpenFolder) => {
                let opened = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| open::that(&dir).map_err(|e| e.to_string()));
                if let Err(e) = opened {
                    self.error = Some(format!("No se pudo abrir {}: {e}", dir.display()));
                }
            }
            None => {}
        }
    }

    fn tools_card(&mut self, ui: &mut egui::Ui) {
        // "9.0.2-full_build-www.gyan.dev" → "9.0.2"
        let ffmpeg = self
            .ffmpeg_status
            .as_ref()
            .map(|r| r.as_ref().map(|v| v.split('-').next().unwrap_or(v).to_string()).map_err(Clone::clone));
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(theme::meta("Herramientas"));
            theme::status(ui, "yt-dlp", self.ytdlp_busy, self.ytdlp_status.as_ref(), "no disponible");
            theme::status(ui, "ffmpeg", self.ffmpeg_busy, ffmpeg.as_ref(), "no disponible");
        });
    }

    fn quota_card(&mut self, ui: &mut egui::Ui) {
        let (used, reset) = {
            let mut q = self.quota.lock().unwrap();
            q.refresh(Utc::now());
            (q.used(), quota::next_reset(Utc::now()))
        };
        theme::card()
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                theme::row(
                    ui,
                    "Cuota de YouTube hoy",
                    theme::semibold(format!("{} %", (used as f32 * 100.0 / DAILY_LIMIT as f32).round()), 13.0),
                );
                theme::progress_bar(ui, used as f32 / DAILY_LIMIT as f32);
                ui.label(theme::meta(format!(
                    "{} de {} · se reinicia a las {}",
                    thousands(used),
                    thousands(DAILY_LIMIT),
                    reset.format("%H:%M")
                )));
            })
            .response
            .on_hover_text(
                "Contador local: suma el coste de cada llamada que hace esta app. \
                 El dato oficial está en Google Cloud Console → Cuotas.",
            );
    }

    fn delete_button(&mut self, ui: &mut egui::Ui, p: &PlaylistResult) {
        let blocker = if self.running {
            Some("Espera a que termine la playlist en curso.".to_string())
        } else if self.deleting.is_some() {
            Some("Ya se está borrando una playlist.".to_string())
        } else if self.config.client_secret_path.trim().is_empty() {
            Some("Falta configurar YouTube en Ajustes.".to_string())
        } else if self.quota.lock().unwrap().remaining() < COST_DELETE {
            Some(format!("Borrar cuesta {COST_DELETE} unidades de cuota y no quedan suficientes."))
        } else {
            None
        };
        if self.deleting.as_deref() == Some(p.playlist_id.as_str()) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::meta("Borrando…"));
            });
            return;
        }
        let resp = ui.add_enabled(blocker.is_none(), theme::danger("Borrar playlist"));
        let resp = match &blocker {
            Some(reason) => resp.on_disabled_hover_text(reason),
            None => resp.on_hover_text(format!("La borra de tu cuenta de YouTube ({COST_DELETE} unidades)")),
        };
        if resp.clicked() {
            self.confirm_delete = Some(p.playlist_id.clone());
        }
    }

    fn confirm_delete_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete.clone() else { return };
        let Some(p) = self.history.iter().find(|p| p.playlist_id == id) else {
            self.confirm_delete = None;
            return;
        };
        let title = p.title.clone();
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new("Borrar playlist")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("¿Borrar \"{title}\" de tu cuenta de YouTube?"));
                ui.label(RichText::new(format!(
                    "No se puede deshacer. También desaparece del historial. Cuesta {COST_DELETE} unidades de cuota."
                )).color(theme::MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let delete = egui::Button::new(theme::semibold("Borrar", 13.0).color(Color32::WHITE))
                        .fill(Color32::from_rgb(190, 50, 50))
                        .stroke(egui::Stroke::NONE)
                        .min_size(egui::vec2(0.0, 32.0));
                    confirmed = ui.add(delete).clicked();
                    cancelled = ui.add(theme::secondary("Cancelar")).clicked();
                });
            });
        if confirmed {
            self.confirm_delete = None;
            self.error = None;
            self.deleting = Some(id.clone());
            self.worker.send(Command::DeletePlaylist(self.config.clone(), id));
        } else if cancelled {
            self.confirm_delete = None;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.settings.take() else { return };
        let mut open = true;
        let mut action = SettingsAction::None;
        let mut check_ytdlp = false;
        let mut update_ytdlp = false;
        let mut connect_youtube = false;
        let mut check_ffmpeg = false;
        let mut update_ffmpeg = false;

        egui::Window::new("Ajustes")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                // Los botones Guardar/Cancelar quedan fuera del scroll, siempre a la vista.
                let max_height = (ui.ctx().content_rect().height() - 150.0).max(200.0);
                egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
                    theme::section(ui, "1", "Inteligencia artificial");
                    egui::Grid::new("llm").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Proveedor:");
                        let before = draft.config.llm.provider;
                        egui::ComboBox::from_id_salt("provider")
                            .selected_text(before.label())
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                for p in Provider::ALL {
                                    ui.selectable_value(&mut draft.config.llm.provider, p, p.label());
                                }
                            });
                        let now = draft.config.llm.provider;
                        if now != before {
                            let (base, model) = now.preset();
                            draft.config.llm.base_url = base.into();
                            draft.config.llm.model = model.into();
                            draft.api_key = config::load_api_key(now);
                        }
                        ui.end_row();

                        ui.label("URL base:");
                        ui.add(egui::TextEdit::singleline(&mut draft.config.llm.base_url).desired_width(320.0));
                        ui.end_row();

                        ui.label("Modelo:");
                        ui.add(egui::TextEdit::singleline(&mut draft.config.llm.model).desired_width(320.0));
                        ui.end_row();

                        if draft.config.llm.provider.needs_api_key() {
                            ui.label("API key:");
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.api_key)
                                    .password(true)
                                    .hint_text("Se guarda en el Administrador de credenciales de Windows")
                                    .desired_width(320.0),
                            );
                            ui.end_row();
                        }
                    });

                    ui.add_space(8.0);
                    theme::section(ui, "2", "yt-dlp (búsqueda y descargas)");
                    ui.horizontal(|ui| {
                        ui.label("Ejecutable:");
                        ui.add(egui::TextEdit::singleline(&mut draft.config.ytdlp_path).desired_width(220.0));
                        if ui.add_enabled(!self.ytdlp_busy, theme::secondary("Comprobar")).clicked() {
                            check_ytdlp = true;
                        }
                        let idle = !self.ytdlp_busy && !self.running && self.files_task.is_none();
                        if ui
                            .add_enabled(
                                idle,
                                theme::secondary(if matches!(self.ytdlp_status, Some(Err(_))) {
                                    "Instalar yt-dlp"
                                } else {
                                    "Actualizar yt-dlp"
                                }),
                            )
                            .on_hover_text("Usa pip, winget o yt-dlp -U según cómo esté instalado")
                            .clicked()
                        {
                            update_ytdlp = true;
                        }
                    });
                    match &self.ytdlp_status {
                        _ if self.ytdlp_busy => {
                            ui.spinner();
                        }
                        Some(Ok(v)) => {
                            ui.weak(format!("Versión instalada: {v}. YouTube cambia a menudo: si falla la búsqueda, actualiza."));
                        }
                        Some(Err(e)) => {
                            ui.colored_label(theme::WARN, e);
                            ui.weak("Instálalo con:  winget install yt-dlp");
                        }
                        None => {}
                    }

                    ui.add_space(8.0);
                    theme::section(ui, "3", "Descargas");
                    ui.horizontal(|ui| {
                        ui.label("Carpeta de descargas:");
                        ui.add(egui::TextEdit::singleline(&mut draft.config.download_dir).desired_width(300.0));
                    });
                    ui.weak("Cada playlist se guarda en su propia subcarpeta.");
                    let has_ffmpeg = matches!(self.ffmpeg_status, Some(Ok(_)));
                    ui.horizontal(|ui| {
                        ui.label("Formato de audio:");
                        egui::ComboBox::from_id_salt("audio_format")
                            .selected_text(draft.config.audio_format.label())
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                for f in AudioFormat::ALL {
                                    ui.add_enabled_ui(has_ffmpeg || !f.needs_ffmpeg(), |ui| {
                                        ui.selectable_value(&mut draft.config.audio_format, f, f.label())
                                            .on_disabled_hover_text("Necesita ffmpeg");
                                    });
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("ffmpeg:");
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.config.ffmpeg_path)
                                .hint_text("Vacío = buscarlo automáticamente")
                                .desired_width(220.0),
                        );
                        if ui.add_enabled(!self.ffmpeg_busy, theme::secondary("Comprobar")).clicked() {
                            check_ffmpeg = true;
                        }
                        let label = if has_ffmpeg { "Actualizar ffmpeg" } else { "Instalar ffmpeg" };
                        if ui
                            .add_enabled(!self.ffmpeg_busy && self.files_task.is_none(), theme::secondary(label))
                            .on_hover_text("Usa winget (paquete Gyan.FFmpeg)")
                            .clicked()
                        {
                            update_ffmpeg = true;
                        }
                    });
                    match &self.ffmpeg_status {
                        _ if self.ffmpeg_busy => {
                            ui.spinner();
                        }
                        Some(Ok(v)) => {
                            ui.weak(format!("ffmpeg {v}: permite MP3 y añade carátula y etiquetas a las descargas."));
                        }
                        Some(Err(e)) => {
                            ui.colored_label(theme::WARN, e);
                        }
                        None => {}
                    }

                    ui.add_space(8.0);
                    theme::section(ui, "4", "YouTube");
                    ui.horizontal(|ui| {
                        ui.label("client_secret.json:");
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.config.client_secret_path)
                                .hint_text(r"C:\ruta\client_secret.json")
                                .desired_width(300.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        let can_connect = !self.youtube_busy && !draft.config.client_secret_path.trim().is_empty();
                        if ui.add_enabled(can_connect, theme::secondary("Conectar con YouTube")).clicked() {
                            connect_youtube = true;
                        }
                        match &self.youtube_status {
                            _ if self.youtube_busy => {
                                ui.spinner();
                                ui.label("Esperando autorización en el navegador…");
                            }
                            Some(Ok(channel)) => {
                                ui.label(theme::meta("Conectado:"));
                                ui.label(theme::readout(channel.as_str()));
                            }
                            Some(Err(e)) => {
                                ui.colored_label(theme::WARN, e);
                            }
                            None => {}
                        }
                    });
                    egui::CollapsingHeader::new("¿Cómo consigo el client_secret.json?").show(ui, |ui| {
                        ui.label(
                            "1. Entra en console.cloud.google.com y crea un proyecto.\n\
                             2. APIs y servicios → Biblioteca → activa \"YouTube Data API v3\".\n\
                             3. Pantalla de consentimiento OAuth → tipo Externo → añade tu cuenta en \"Usuarios de prueba\".\n\
                             4. Credenciales → Crear credenciales → ID de cliente OAuth → tipo \"App de escritorio\".\n\
                             5. Descarga el JSON y escribe aquí su ruta.\n\n\
                             En modo de prueba, Google pide volver a autorizar cada 7 días: la app abrirá el navegador cuando haga falta.",
                        );
                    });
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("Guardar")).clicked() {
                        action = SettingsAction::Save;
                    }
                    if ui.add(theme::secondary("Cancelar")).clicked() {
                        action = SettingsAction::Cancel;
                    }
                });
            });

        if check_ytdlp || update_ytdlp {
            self.ytdlp_busy = true;
            let bin = draft.config.ytdlp_path.trim().to_string();
            self.worker.send(if update_ytdlp {
                Command::UpdateYtdlp(bin)
            } else {
                Command::CheckYtdlp(bin)
            });
        }
        if check_ffmpeg || update_ffmpeg {
            self.ffmpeg_busy = true;
            let path = draft.config.ffmpeg_path.clone();
            self.worker.send(if update_ffmpeg { Command::UpdateFfmpeg(path) } else { Command::CheckFfmpeg(path) });
        }
        if connect_youtube {
            self.youtube_busy = true;
            self.worker.send(Command::ConnectYoutube(draft.config.clone()));
        }

        match action {
            SettingsAction::Save => {
                let ytdlp_changed = draft.config.ytdlp_path != self.config.ytdlp_path;
                if draft.config.ffmpeg_path != self.config.ffmpeg_path && !self.ffmpeg_busy {
                    self.ffmpeg_busy = true;
                    self.worker.send(Command::CheckFfmpeg(draft.config.ffmpeg_path.clone()));
                }
                self.config = draft.config;
                self.api_key = draft.api_key.trim().to_string();
                let mut errors = Vec::new();
                if let Err(e) = self.config.save() {
                    errors.push(format!("{e:#}"));
                }
                if self.config.llm.provider.needs_api_key()
                    && let Err(e) = config::save_api_key(self.config.llm.provider, &self.api_key)
                {
                    errors.push(format!("{e:#}"));
                }
                self.error = (!errors.is_empty()).then(|| errors.join(" · "));
                if ytdlp_changed && !self.ytdlp_busy {
                    self.ytdlp_busy = true;
                    self.worker.send(Command::CheckYtdlp(self.config.ytdlp_path.clone()));
                }
            }
            SettingsAction::Cancel => {}
            SettingsAction::None if open => self.settings = Some(draft),
            SettingsAction::None => {}
        }
    }
}

impl eframe::App for PlaylistApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_events();

        // Sin playlist elegida se muestra la pantalla "Nueva playlist".
        let playlist = self.selected.and_then(|i| self.history.get(i)).cloned();
        let frame = |fill, x, y| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(x, y));
        egui::Panel::top("top")
            .frame(frame(theme::SIDEBAR, 12, 6))
            .show(ui, |ui| self.top_bar(ui, playlist.as_ref()));
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(206.0)
            .frame(frame(theme::SIDEBAR, 8, 10))
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::right("info")
            .resizable(false)
            .exact_size(272.0)
            .frame(frame(theme::BG, 14, 16))
            .show(ui, |ui| self.info_panel(ui, playlist.as_ref()));
        egui::CentralPanel::default().frame(frame(theme::BG, 22, 14)).show(ui, |ui| match &playlist {
            Some(p) => self.playlist_view(ui, p),
            None => self.new_view(ui),
        });

        let ctx = ui.ctx().clone();
        self.settings_window(&ctx);
        self.confirm_delete_window(&ctx);
    }
}

/// Etapa de la creación (0 a 4) que indica un mensaje de progreso del agente.
fn creation_stage(line: &str) -> Option<usize> {
    [
        ("Pidiendo a", 0),
        ("Lista de", 1),
        ("Buscando", 1),
        ("Eligiendo", 2),
        ("Creando la playlist", 3),
        ("Añadiendo", 3),
        ("Playlist creada", 4),
    ]
    .into_iter()
    .find(|(prefix, _)| line.starts_with(prefix))
    .map(|(_, stage)| stage)
}

/// 10000 → "10.000"
fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn etapa_de_creacion_segun_el_mensaje() {
        assert_eq!(super::creation_stage("Pidiendo a DeepSeek las 10 canciones…"), Some(0));
        assert_eq!(super::creation_stage("Buscando 3/10: Iris"), Some(1));
        assert_eq!(super::creation_stage("Añadiendo 2/10: Iris"), Some(3));
        assert_eq!(super::creation_stage("Playlist creada con 10 canciones."), Some(4));
        assert_eq!(super::creation_stage("Descargando 1/10: Iris"), None);
    }

    #[test]
    fn separador_de_miles() {
        assert_eq!(super::thousands(10_000), "10.000");
        assert_eq!(super::thousands(550), "550");
        assert_eq!(super::thousands(1_234_567), "1.234.567");
    }
}

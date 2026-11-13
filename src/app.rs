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
                        Err(msg) => self.error = Some(format!("No se pudo actualizar yt-dlp: {msg}")),
                    }
                    self.worker.send(Command::CheckYtdlp(self.config.ytdlp_path.clone()));
                }
                Event::FfmpegUpdated(result) => {
                    match result {
                        Ok(msg) => self.progress.push(msg),
                        Err(msg) => self.error = Some(format!("No se pudo actualizar ffmpeg: {msg}")),
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("PLAYLISTYT").monospace().size(15.0).strong().color(theme::TEXT));
            ui.label(theme::meta("/  AGENTE DE PLAYLISTS"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::secondary("Ajustes")).clicked() && self.settings.is_none() {
                    self.settings = Some(SettingsDraft {
                        config: self.config.clone(),
                        api_key: self.api_key.clone(),
                    });
                }
            });
        });
    }

    /// Barra inferior con el estado de las herramientas y la cuota.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        // "9.0.2-full_build-www.gyan.dev" → "9.0.2"
        let ffmpeg = self
            .ffmpeg_status
            .as_ref()
            .map(|r| r.as_ref().map(|v| v.split('-').next().unwrap_or(v).to_string()).map_err(Clone::clone));
        ui.horizontal(|ui| {
            theme::status(ui, "yt-dlp", self.ytdlp_busy, self.ytdlp_status.as_ref(), "no disponible");
            ui.separator();
            theme::status(ui, "ffmpeg", self.ffmpeg_busy, ffmpeg.as_ref(), "no disponible");
            ui.separator();
            theme::status(ui, "YouTube", self.youtube_busy, self.youtube_status.as_ref(), "sin conectar");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (used, reset) = {
                    let mut q = self.quota.lock().unwrap();
                    q.refresh(Utc::now());
                    (q.used(), quota::next_reset(Utc::now()))
                };
                ui.label(theme::meta(format!("REINICIO {}", reset.format("%H:%M"))));
                let resp = ui.label(theme::readout(format!("{} / {}", thousands(used), thousands(DAILY_LIMIT))));
                ui.label(theme::meta("CUOTA"));
                resp.on_hover_text(
                    "Contador local: suma el coste de cada llamada que hace esta app. \
                     El dato oficial está en Google Cloud Console → Cuotas.",
                );
            });
        });
    }

    fn history_panel(&mut self, ui: &mut egui::Ui) {
        theme::panel_header(ui, &format!("Historial  ·  {}", self.history.len()));
        if self.history.is_empty() {
            ui.label(theme::meta("Aún no has creado playlists."));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, p) in self.history.iter().enumerate() {
                let selected = self.selected == Some(i);
                let mut job = egui::text::LayoutJob::default();
                let title_color = if selected { theme::TEXT } else { Color32::from_rgb(214, 213, 210) };
                job.append(&p.title, 0.0, egui::TextFormat::simple(egui::FontId::proportional(13.5), title_color));
                job.append(
                    &format!("\n{} · {} CANCIONES", p.created_at.format("%d/%m/%Y"), p.items.len()),
                    0.0,
                    egui::TextFormat::simple(egui::FontId::monospace(10.0), theme::MUTED),
                );
                let button = egui::Button::selectable(selected, job).min_size(egui::vec2(ui.available_width(), 40.0));
                if ui.add(button).clicked() {
                    self.selected = Some(i);
                }
            }
        });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "01", "Nueva playlist");
        egui::Grid::new("form").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(theme::meta("CANTANTE"));
            ui.add(
                egui::TextEdit::singleline(&mut self.artist)
                    .hint_text("Ej.: Queen, Shakira, Bad Bunny")
                    .desired_width(300.0),
            );
            ui.end_row();

            ui.label(theme::meta("Nº DE CANCIONES"));
            ui.add(egui::DragValue::new(&mut self.count).range(1..=MAX_SONGS));
            ui.end_row();
        });

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

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let resp = ui.add_enabled(blocker.is_none(), theme::primary("Crear playlist"));
            if resp.clicked() {
                self.start_creation();
            }
            if self.running {
                ui.spinner();
            }
            ui.label(theme::meta("COSTE"));
            ui.label(theme::readout(format!("{} U.", thousands(cost))));
            ui.label(theme::meta("·  IA"));
            ui.label(theme::readout(self.config.llm.provider.label().to_uppercase()));
        });
        if let Some(reason) = blocker.filter(|_| !self.running) {
            ui.label(RichText::new(reason).color(theme::MUTED));
        }
    }

    fn progress_view(&mut self, ui: &mut egui::Ui) {
        if self.progress.is_empty() && self.error.is_none() {
            return;
        }
        ui.add_space(8.0);
        theme::section(ui, "02", "Progreso del agente");
        if let Some(err) = &self.error {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("ERROR").monospace().size(11.0).color(theme::DANGER));
                ui.label(RichText::new(err).color(theme::DANGER));
            });
        }
        if self.progress.is_empty() {
            return;
        }
        theme::inset().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(110.0)
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
                        ui.label(RichText::new(format!("> {line}")).monospace().size(12.0).color(color));
                    }
                });
        });
    }

    fn playlist_view(&mut self, ui: &mut egui::Ui) {
        let delete_blocker = if self.running {
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
        ui.add_space(8.0);
        theme::section(ui, "03", "Playlist");
        let Some(p) = self.selected.and_then(|i| self.history.get(i)) else {
            ui.label(theme::meta("Aquí aparecerá la playlist creada por el agente."));
            return;
        };
        let url = p.url();
        let mut ask_delete = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.title).heading().strong().color(theme::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.deleting.as_deref() == Some(p.playlist_id.as_str()) {
                    ui.label(theme::meta("BORRANDO…"));
                    ui.spinner();
                } else {
                    let resp = ui.add_enabled(delete_blocker.is_none(), theme::danger("Borrar playlist"));
                    let resp = match &delete_blocker {
                        Some(reason) => resp.on_disabled_hover_text(reason),
                        None => resp.on_hover_text(format!("La borra de tu cuenta de YouTube ({COST_DELETE} unidades)")),
                    };
                    ask_delete = resp.clicked();
                }
                if ui.add(theme::secondary("Copiar link")).clicked() {
                    ui.ctx().copy_text(url.clone());
                }
            });
        });
        ui.label(theme::meta(format!(
            "{} CANCIONES  ·  OCULTA (UNLISTED)  ·  CREADA EL {}",
            p.items.len(),
            p.created_at.format("%d/%m/%Y %H:%M")
        )));
        ui.hyperlink_to(RichText::new(&url).monospace().size(12.5), &url);
        if ask_delete {
            self.confirm_delete = Some(p.playlist_id.clone());
        }
        if !p.missing.is_empty() {
            ui.colored_label(theme::WARN, format!("No encontradas o no añadidas: {}", p.missing.join(", ")));
        }
        ui.add_space(4.0);

        let dir = download::playlist_dir(&self.config.download_dir, p);
        let downloaded = download::Manifest::load(&dir).downloaded(&dir, &p.items);
        let busy = self.running || self.files_task.is_some();
        let mut files_action = None;
        ui.horizontal(|ui| {
            match &self.files_task {
                Some((id, label)) if *id == p.playlist_id => {
                    ui.spinner();
                    ui.label(theme::meta(label.to_uppercase()));
                }
                _ => {
                    let format = self.config.audio_format;
                    let has_ffmpeg = matches!(self.ffmpeg_status, Some(Ok(_)));
                    let resp = ui.add_enabled(
                        !busy && (has_ffmpeg || !format.needs_ffmpeg()),
                        theme::primary(&format!("Descargar en {}", format.extension())),
                    );
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
            if ui.add(theme::secondary("Abrir carpeta")).clicked() {
                files_action = Some(FilesAction::OpenFolder);
            }
            ui.label(theme::readout(format!("{downloaded} / {}", p.items.len())));
            ui.label(theme::meta("DESCARGADAS"));
        });
        ui.label(RichText::new("Descarga solo música sobre la que tengas derechos.").small().color(theme::MUTED));
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
        ui.add_space(6.0);

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::initial(180.0).at_least(100.0).clip(true))
            .column(Column::initial(220.0).at_least(110.0).clip(true))
            .column(Column::initial(130.0).at_least(80.0).clip(true))
            .column(Column::exact(52.0))
            .column(Column::remainder().at_least(120.0))
            .header(24.0, |mut h| {
                for title in ["#", "Canción", "Vídeo en YouTube", "Canal", "Dur.", "Tipo"] {
                    h.col(|ui| {
                        ui.label(theme::meta(title.to_uppercase()));
                    });
                }
            })
            .body(|mut body| {
                for (i, item) in p.items.iter().enumerate() {
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            ui.label(theme::meta(format!("{:02}", i + 1)));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&item.song).color(theme::TEXT));
                        });
                        row.col(|ui| {
                            ui.hyperlink_to(&item.video_title, item.url());
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(item.channel.as_deref().unwrap_or("YouTube Music")).color(theme::MUTED));
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(item.duration_secs.map(agent::format_duration).unwrap_or_default())
                                    .monospace()
                                    .color(theme::MUTED),
                            );
                        });
                        row.col(|ui| {
                            theme::kind_badge(ui, item.kind);
                        });
                    });
                }
            });
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
                    let delete = egui::Button::new(RichText::new("BORRAR").monospace().size(12.0).strong().color(Color32::WHITE))
                        .fill(Color32::from_rgb(170, 40, 40))
                        .min_size(egui::vec2(0.0, 30.0));
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
                    theme::section(ui, "01", "Inteligencia artificial");
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
                    theme::section(ui, "02", "yt-dlp (búsqueda y descargas)");
                    ui.horizontal(|ui| {
                        ui.label("Ejecutable:");
                        ui.add(egui::TextEdit::singleline(&mut draft.config.ytdlp_path).desired_width(220.0));
                        if ui.add_enabled(!self.ytdlp_busy, theme::secondary("Comprobar")).clicked() {
                            check_ytdlp = true;
                        }
                        let idle = !self.ytdlp_busy && !self.running && self.files_task.is_none();
                        if ui
                            .add_enabled(idle, theme::secondary("Actualizar yt-dlp"))
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
                    theme::section(ui, "03", "Descargas");
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
                    theme::section(ui, "04", "YouTube");
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
                                ui.label(theme::meta("CONECTADO"));
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

        let bar = |fill| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(14, 8));
        egui::Panel::top("top").frame(bar(theme::BG)).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").frame(bar(theme::INSET)).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("history")
            .resizable(true)
            .default_size(220.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| self.history_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(22, 14)))
            .show(ui, |ui| {
                self.form(ui);
                self.progress_view(ui);
                self.playlist_view(ui);
            });

        let ctx = ui.ctx().clone();
        self.settings_window(&ctx);
        self.confirm_delete_window(&ctx);
    }
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
    fn separador_de_miles() {
        assert_eq!(super::thousands(10_000), "10.000");
        assert_eq!(super::thousands(550), "550");
        assert_eq!(super::thousands(1_234_567), "1.234.567");
    }
}

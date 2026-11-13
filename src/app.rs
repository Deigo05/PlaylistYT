//! Interfaz: formulario (cantante + nº de canciones), progreso, playlist creada,
//! historial y ventana de ajustes.

use crate::agent::{self, MAX_SONGS, Request};
use crate::config::{self, Config, Provider};
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
}

impl PlaylistApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = Config::load();
        let api_key = config::load_api_key(config.llm.provider);
        let quota = Arc::new(Mutex::new(Quota::load()));
        let worker = Worker::start(cc.egui_ctx.clone(), quota.clone());
        let history = history::load();

        worker.send(Command::CheckYtdlp(config.ytdlp_path.clone()));
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
                Event::YoutubeConnected(result) => {
                    self.youtube_busy = false;
                    self.youtube_status = Some(result);
                }
                Event::YtdlpVersion(result) => {
                    self.ytdlp_busy = false;
                    self.ytdlp_status = Some(result);
                }
                Event::YtdlpUpdated(result) => {
                    self.ytdlp_busy = false;
                    match result {
                        Ok(msg) => {
                            self.progress.push(format!("yt-dlp: {msg}"));
                            self.ytdlp_busy = true;
                            self.worker.send(Command::CheckYtdlp(self.config.ytdlp_path.clone()));
                        }
                        Err(msg) => self.ytdlp_status = Some(Err(msg)),
                    }
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
            ui.heading("PlaylistYT");
            ui.separator();

            match &self.ytdlp_status {
                _ if self.ytdlp_busy => {
                    ui.spinner();
                    ui.label("yt-dlp…");
                }
                Some(Ok(v)) => {
                    ui.label(format!("yt-dlp {v}"));
                }
                Some(Err(e)) => {
                    ui.colored_label(Color32::from_rgb(220, 120, 0), "yt-dlp no disponible")
                        .on_hover_text(e);
                }
                None => {}
            }
            ui.separator();
            match &self.youtube_status {
                _ if self.youtube_busy => {
                    ui.spinner();
                    ui.label("Conectando con YouTube…");
                }
                Some(Ok(channel)) => {
                    ui.label(format!("YouTube: {channel}"));
                }
                Some(Err(e)) => {
                    ui.colored_label(Color32::from_rgb(220, 120, 0), "YouTube sin conectar")
                        .on_hover_text(e);
                }
                None => {
                    ui.label("YouTube sin conectar");
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Ajustes").clicked() && self.settings.is_none() {
                    self.settings = Some(SettingsDraft {
                        config: self.config.clone(),
                        api_key: self.api_key.clone(),
                    });
                }
                let (used, reset) = {
                    let mut q = self.quota.lock().unwrap();
                    q.refresh(Utc::now());
                    (q.used(), quota::next_reset(Utc::now()))
                };
                ui.label(format!(
                    "Cuota YouTube: {} / {} · se reinicia a las {}",
                    thousands(used),
                    thousands(DAILY_LIMIT),
                    reset.format("%H:%M")
                ))
                .on_hover_text(
                    "Contador local: suma el coste de cada llamada que hace esta app. \
                     El dato oficial está en Google Cloud Console → Cuotas.",
                );
            });
        });
    }

    fn history_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Historial");
        ui.separator();
        if self.history.is_empty() {
            ui.weak("Aún no has creado playlists.");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, p) in self.history.iter().enumerate() {
                let text = format!("{}\n{}", p.title, p.created_at.format("%d/%m/%Y %H:%M"));
                if ui
                    .selectable_label(self.selected == Some(i), text)
                    .clicked()
                {
                    self.selected = Some(i);
                }
            }
        });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Cantante:");
            ui.add(
                egui::TextEdit::singleline(&mut self.artist)
                    .hint_text("Ej.: Queen, Shakira, Bad Bunny")
                    .desired_width(280.0),
            );
            ui.end_row();

            ui.label("Nº de canciones más populares:");
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
            let button = egui::Button::new(RichText::new("Crear playlist").strong());
            let resp = ui.add_enabled(blocker.is_none(), button);
            if resp.clicked() {
                self.start_creation();
            }
            if self.running {
                ui.spinner();
            }
            ui.label(format!(
                "Coste estimado: {} unidades de cuota · IA: {}",
                thousands(cost),
                self.config.llm.provider.label()
            ));
        });
        if let Some(reason) = blocker.filter(|_| !self.running) {
            ui.weak(reason);
        }
    }

    fn progress_view(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            ui.add_space(4.0);
            ui.colored_label(Color32::from_rgb(220, 60, 60), format!("Error: {err}"));
        }
        if self.progress.is_empty() {
            return;
        }
        egui::CollapsingHeader::new("Progreso del agente")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(130.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.progress {
                            ui.label(line);
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
        let Some(p) = self.selected.and_then(|i| self.history.get(i)) else {
            ui.weak("Aquí aparecerá la playlist creada por el agente.");
            return;
        };
        ui.heading(&p.title);
        let url = p.url();
        let mut ask_delete = false;
        ui.horizontal(|ui| {
            ui.hyperlink_to(&url, &url);
            if ui.button("Copiar link").clicked() {
                ui.ctx().copy_text(url.clone());
            }
            if self.deleting.as_deref() == Some(p.playlist_id.as_str()) {
                ui.spinner();
                ui.label("Borrando…");
            } else {
                let resp = ui.add_enabled(delete_blocker.is_none(), egui::Button::new("Borrar playlist"));
                let resp = match &delete_blocker {
                    Some(reason) => resp.on_disabled_hover_text(reason),
                    None => resp.on_hover_text(format!("La borra de tu cuenta de YouTube ({COST_DELETE} unidades)")),
                };
                ask_delete = resp.clicked();
            }
        });
        if ask_delete {
            self.confirm_delete = Some(p.playlist_id.clone());
        }
        ui.weak(format!(
            "{} canciones · oculta (unlisted) · creada el {}",
            p.items.len(),
            p.created_at.format("%d/%m/%Y %H:%M")
        ));
        if !p.missing.is_empty() {
            ui.colored_label(
                Color32::from_rgb(220, 120, 0),
                format!("No encontradas o no añadidas: {}", p.missing.join(", ")),
            );
        }
        ui.add_space(6.0);

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::initial(200.0).at_least(100.0))
            .column(Column::initial(280.0).at_least(120.0).clip(true))
            .column(Column::initial(150.0).clip(true))
            .column(Column::exact(56.0))
            .column(Column::remainder())
            .header(22.0, |mut h| {
                for title in ["#", "Canción", "Vídeo en YouTube", "Canal", "Dur.", "Tipo"] {
                    h.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|mut body| {
                for (i, item) in p.items.iter().enumerate() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label((i + 1).to_string());
                        });
                        row.col(|ui| {
                            ui.label(&item.song);
                        });
                        row.col(|ui| {
                            ui.hyperlink_to(&item.video_title, item.url());
                        });
                        row.col(|ui| {
                            ui.label(item.channel.as_deref().unwrap_or("YouTube Music"));
                        });
                        row.col(|ui| {
                            ui.label(item.duration_secs.map(agent::format_duration).unwrap_or_default());
                        });
                        row.col(|ui| {
                            ui.label(item.kind.label());
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
                ui.weak(format!(
                    "No se puede deshacer. También desaparece del historial. Cuesta {COST_DELETE} unidades de cuota."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let delete = egui::Button::new(RichText::new("Borrar").color(Color32::WHITE).strong())
                        .fill(Color32::from_rgb(170, 40, 40));
                    confirmed = ui.add(delete).clicked();
                    cancelled = ui.button("Cancelar").clicked();
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

        egui::Window::new("Ajustes")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.heading("Inteligencia artificial");
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

                ui.separator();
                ui.heading("yt-dlp (búsqueda de canciones)");
                ui.horizontal(|ui| {
                    ui.label("Ejecutable:");
                    ui.add(egui::TextEdit::singleline(&mut draft.config.ytdlp_path).desired_width(220.0));
                    if ui.add_enabled(!self.ytdlp_busy, egui::Button::new("Comprobar")).clicked() {
                        check_ytdlp = true;
                    }
                    if ui.add_enabled(!self.ytdlp_busy, egui::Button::new("Actualizar yt-dlp")).clicked() {
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
                        ui.colored_label(Color32::from_rgb(220, 120, 0), e);
                        ui.weak("Instálalo con:  winget install yt-dlp");
                    }
                    None => {}
                }

                ui.separator();
                ui.heading("YouTube");
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
                    if ui.add_enabled(can_connect, egui::Button::new("Conectar con YouTube")).clicked() {
                        connect_youtube = true;
                    }
                    match &self.youtube_status {
                        _ if self.youtube_busy => {
                            ui.spinner();
                            ui.label("Esperando autorización en el navegador…");
                        }
                        Some(Ok(channel)) => {
                            ui.label(format!("Conectado: {channel}"));
                        }
                        Some(Err(e)) => {
                            ui.colored_label(Color32::from_rgb(220, 120, 0), e);
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

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Guardar").strong()).clicked() {
                        action = SettingsAction::Save;
                    }
                    if ui.button("Cancelar").clicked() {
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
        if connect_youtube {
            self.youtube_busy = true;
            self.worker.send(Command::ConnectYoutube(draft.config.clone()));
        }

        match action {
            SettingsAction::Save => {
                let ytdlp_changed = draft.config.ytdlp_path != self.config.ytdlp_path;
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

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("history")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.history_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            self.form(ui);
            self.progress_view(ui);
            ui.separator();
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

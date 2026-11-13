//! Hilo de fondo con un runtime de tokio. La interfaz le manda órdenes y recibe eventos
//! sin bloquearse.

use crate::agent::{self, Request};
use crate::config::Config;
use crate::{download, ffmpeg};
use crate::history::PlaylistResult;
use crate::llm::LlmClient;
use crate::quota::Quota;
use crate::youtube::YouTube;
use crate::ytdlp;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub enum Command {
    Create(Request),
    ConnectYoutube(Config),
    CheckYtdlp(String),
    UpdateYtdlp(String),
    /// Borra de YouTube la playlist con ese id.
    DeletePlaylist(Config, String),
    /// Busca ffmpeg (ruta configurada o vacía para buscarlo).
    CheckFfmpeg(String),
    /// Descarga con yt-dlp el audio de la playlist.
    Download(Config, PlaylistResult),
    /// Renombra con el LLM los archivos descargados de la playlist.
    CleanNames(Config, String, PlaylistResult),
}

pub enum Event {
    Progress(String),
    Done(PlaylistResult),
    Failed(String),
    PlaylistDeleted {
        playlist_id: String,
        result: Result<(), String>,
    },
    /// Versión de ffmpeg encontrada, o por qué no se encontró.
    FfmpegFound(Result<String, String>),
    /// Fin de una descarga o de una limpieza de nombres.
    FilesTaskDone(Result<String, String>),
    YoutubeConnected(Result<String, String>),
    YtdlpVersion(Result<String, String>),
    YtdlpUpdated(Result<String, String>),
}

pub struct Worker {
    commands: UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
}

impl Worker {
    pub fn start(ctx: eframe::egui::Context, quota: Arc<Mutex<Quota>>) -> Self {
        let (cmd_tx, mut cmd_rx) = unbounded_channel::<Command>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let emit = move |event: Event| {
            let _ = ev_tx.send(event);
            ctx.request_repaint();
        };
        let emit = Arc::new(emit);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime de tokio");
            rt.block_on(async move {
                while let Some(cmd) = cmd_rx.recv().await {
                    let emit = emit.clone();
                    let quota = quota.clone();
                    tokio::spawn(async move { handle(cmd, quota, emit).await });
                }
            });
        });

        Self {
            commands: cmd_tx,
            events: ev_rx,
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }

    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}

fn progress_for(emit: &Arc<impl Fn(Event) + Send + Sync + 'static>) -> agent::Progress {
    let emit = emit.clone();
    Arc::new(move |msg| emit(Event::Progress(msg)))
}

async fn handle(cmd: Command, quota: Arc<Mutex<Quota>>, emit: Arc<impl Fn(Event) + Send + Sync + 'static>) {
    match cmd {
        Command::Create(req) => {
            let progress = progress_for(&emit);
            match agent::create_playlist(req, quota, progress).await {
                Ok(result) => emit(Event::Done(result)),
                Err(e) => emit(Event::Failed(format!("{e:#}"))),
            }
        }
        Command::ConnectYoutube(config) => {
            let result = async {
                let yt = YouTube::connect(&config, quota).await?;
                yt.channel_title().await
            }
            .await
            .map_err(|e| format!("{e:#}"));
            emit(Event::YoutubeConnected(result));
        }
        Command::CheckYtdlp(bin) => {
            emit(Event::YtdlpVersion(ytdlp::version(&bin).await.map_err(|e| format!("{e:#}"))));
        }
        Command::UpdateYtdlp(bin) => {
            emit(Event::YtdlpUpdated(ytdlp::update(&bin).await.map_err(|e| format!("{e:#}"))));
        }
        Command::DeletePlaylist(config, playlist_id) => {
            let result = async {
                let yt = YouTube::connect(&config, quota).await?;
                yt.delete_playlist(&playlist_id).await
            }
            .await
            .map_err(|e| format!("{e:#}"));
            emit(Event::PlaylistDeleted { playlist_id, result });
        }
        Command::CheckFfmpeg(path) => {
            let result = ffmpeg::find(&path).await.map(|f| f.version).map_err(|e| format!("{e:#}"));
            emit(Event::FfmpegFound(result));
        }
        Command::Download(config, playlist) => {
            let progress = progress_for(&emit);
            let result = download::download_playlist(&config, &playlist, progress)
                .await
                .map_err(|e| format!("{e:#}"));
            emit(Event::FilesTaskDone(result));
        }
        Command::CleanNames(config, api_key, playlist) => {
            let progress = progress_for(&emit);
            let result = async {
                let llm = LlmClient::new(config.llm.clone(), api_key)?;
                download::clean_names(&llm, &playlist, &config.download_dir, progress).await
            }
            .await
            .map_err(|e| format!("{e:#}"));
            emit(Event::FilesTaskDone(result));
        }
    }
}

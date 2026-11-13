//! Hilo de fondo con un runtime de tokio. La interfaz le manda órdenes y recibe eventos
//! sin bloquearse.

use crate::agent::{self, Request};
use crate::config::Config;
use crate::history::PlaylistResult;
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
}

pub enum Event {
    Progress(String),
    Done(PlaylistResult),
    Failed(String),
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

async fn handle(cmd: Command, quota: Arc<Mutex<Quota>>, emit: Arc<impl Fn(Event) + Send + Sync + 'static>) {
    match cmd {
        Command::Create(req) => {
            let progress_emit = emit.clone();
            let progress: agent::Progress = Arc::new(move |msg| progress_emit(Event::Progress(msg)));
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
    }
}

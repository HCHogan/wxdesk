//! wxdesk: drives WeChat for Linux in the X display it runs in, and
//! serves what it reads over TCP, one JSON value a line (see
//! `wxdesk-proto`).
//!
//! Configured by the environment:
//! - `WXDESK_GROUP`: the group the driver keeps open (required);
//! - `WXDESK_ME`: the bot's display name, if known before it speaks;
//! - `WXDESK_LISTEN`: where to listen, `0.0.0.0:7710` by default;
//! - `WXDESK_TOKEN_FILE`: a file whose content a client must send first.

mod a11y;
mod input;
mod ui;

use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};
use wxdesk_proto::{FromDriver, ToDriver};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    let group = std::env::var("WXDESK_GROUP").context("WXDESK_GROUP")?;
    let me = std::env::var("WXDESK_ME").ok().filter(|m| !m.is_empty());
    let listen = std::env::var("WXDESK_LISTEN").unwrap_or_else(|_| "0.0.0.0:7710".into());
    let token = match std::env::var("WXDESK_TOKEN_FILE") {
        Ok(path) => Some(
            std::fs::read_to_string(&path)
                .with_context(|| format!("reading {path}"))?
                .trim()
                .to_owned(),
        ),
        Err(_) => None,
    };
    let home = PathBuf::from(std::env::var("HOME").context("HOME")?);

    let (a, app) = loop {
        match connect().await {
            Ok(found) => break found,
            Err(e) => {
                warn!("{e:#}; again in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    info!(?app, "found WeChat");

    let shared = Arc::new(Mutex::new(ui::Shared::default()));
    let status = Arc::new(Mutex::new(ui::Status::default()));
    let (out, mut out_rx) = mpsc::unbounded_channel();
    let (commands, commands_rx) = mpsc::unbounded_channel();
    let (poke, pokes) = mpsc::channel(1);
    let (events, _) = broadcast::channel(1024);

    // Messages wait here while no client is connected, as when the
    // client restarts.
    let backlog = Arc::new(Mutex::new(VecDeque::new()));
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);

    let (fan, kept) = (events.clone(), backlog.clone());
    tokio::spawn(async move {
        while let Some(e) = out_rx.recv().await {
            let mut kept = kept.lock().unwrap();
            if fan.receiver_count() > 0 {
                let _ = fan.send(e);
            } else if matches!(e, FromDriver::Message(_)) {
                kept.push_back(e);
                while kept.len() > BACKLOG {
                    kept.pop_front();
                }
            }
        }
    });
    let watcher = tokio::spawn(ui::watch(a.clone(), app.clone(), shared.clone(), poke));
    let cfg = ui::Config {
        group: group.clone(),
        me,
        home,
    };
    let ui = ui::Ui::new(a, app, cfg, shared, status.clone(), out);
    let window = tokio::spawn(ui.run(commands_rx, pokes));

    let listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("listening on {listen}"))?;
    info!(listen, "serving");
    let server = async {
        loop {
            let (stream, peer) = listener.accept().await?;
            info!(%peer, "a client");
            let conn = Conn {
                run,
                token: token.clone(),
                group: group.clone(),
                status: status.clone(),
                events: events.clone(),
                backlog: backlog.clone(),
                commands: commands.clone(),
            };
            tokio::spawn(async move {
                if let Err(e) = conn.serve(stream).await {
                    warn!(%peer, "{e:#}");
                }
                info!(%peer, "gone");
            });
        }
    };
    tokio::select! {
        r = server => r,
        r = watcher => r?.context("watching events"),
        r = window => r?.context("driving the window"),
    }
}

/// The accessibility bus and WeChat on it, once both are up.
async fn connect() -> Result<(a11y::A11y, a11y::Obj)> {
    let a = a11y::A11y::connect().await?;
    let app = a.app("wechat").await?;
    Ok((a, app))
}

/// Messages kept while no client is connected, at most.
const BACKLOG: usize = 500;

struct Conn {
    run: u64,
    token: Option<String>,
    group: String,
    status: Arc<Mutex<ui::Status>>,
    events: broadcast::Sender<FromDriver>,
    backlog: Arc<Mutex<VecDeque<FromDriver>>>,
    commands: mpsc::UnboundedSender<ToDriver>,
}

impl Conn {
    async fn serve(self, stream: TcpStream) -> Result<()> {
        stream.set_nodelay(true)?;
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        if let Some(token) = &self.token {
            let first = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
                .await
                .context("no token within 10s")??
                .context("hung up before the token")?;
            match serde_json::from_str(&first) {
                Ok(ToDriver::Auth { token: t }) if t == *token => {}
                _ => anyhow::bail!("a wrong token"),
            }
        }
        let hello = {
            let st = self.status.lock().unwrap();
            FromDriver::Hello {
                run: self.run,
                me: st.me.clone(),
                group: self.group.clone(),
                logged_in: st.logged_in,
            }
        };
        // Subscribed with the backlog held, so what comes after it comes
        // to this client.
        let (mut events, kept) = {
            let mut backlog = self.backlog.lock().unwrap();
            (self.events.subscribe(), std::mem::take(&mut *backlog))
        };
        send(&mut write, &hello).await?;
        for e in &kept {
            send(&mut write, e).await?;
        }
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else { return Ok(()) };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<ToDriver>(&line) {
                        Ok(c) => {
                            let _ = self.commands.send(c);
                        }
                        Err(e) => warn!("a line that isn't a command: {e}"),
                    }
                }
                e = events.recv() => match e {
                    Ok(e) => send(&mut write, &e).await?,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "a client fell behind; events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
            }
        }
    }
}

async fn send(write: &mut tokio::net::tcp::OwnedWriteHalf, e: &FromDriver) -> Result<()> {
    let mut line = serde_json::to_vec(e)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    Ok(())
}

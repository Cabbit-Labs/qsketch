//! The socket to a running Leyline: a background thread that keeps a loopback
//! TCP connection to Leyline's sketch link (newline-delimited JSON), reconnects
//! when Leyline is not there yet, and hands parsed messages to the UI thread.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Leyline's default sketch-link port (`LEYLINE_SKETCH_LINK_PORT` on its side,
/// `QSKETCH_LEYLINE_PORT` on ours).
pub const DEFAULT_PORT: u16 = 8952;

#[derive(Clone, Debug, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: bool,
}

/// A canvas op from another participant (or from one of my other devices).
#[derive(Clone, Debug, Deserialize)]
pub struct RemoteOp {
    pub conv: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub lc: u64,
    pub kind: u8,
    #[serde(default)]
    pub points: Vec<f32>,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub text: String,
    /// Base64 as it arrives; decoded on demand by [`RemoteOp::bytes`].
    #[serde(default)]
    pub image: Option<String>,
}

impl RemoteOp {
    pub fn bytes(&self) -> Option<Vec<u8>> {
        let b = self.image.as_ref()?;
        data_encoding::BASE64.decode(b.as_bytes()).ok()
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t")]
pub enum Inbound {
    #[serde(rename = "me")]
    Me { id: String, name: String },
    #[serde(rename = "list")]
    List { items: Vec<Conversation> },
    #[serde(rename = "op")]
    Op(RemoteOp),
    #[serde(rename = "joined")]
    Joined { conv: String },
    #[serde(rename = "sent")]
    Sent { r#ref: u64, lc: u64 },
    #[serde(rename = "error")]
    Error { msg: String },
    /// Synthesized by the thread, not by Leyline.
    #[serde(skip)]
    Connected,
    #[serde(skip)]
    Disconnected,
}

#[derive(Serialize)]
#[serde(tag = "t")]
pub enum Outbound {
    #[serde(rename = "hello")]
    Hello {},
    #[serde(rename = "list")]
    List {},
    #[serde(rename = "join")]
    Join { conv: String },
    #[serde(rename = "leave")]
    Leave { conv: String },
    #[serde(rename = "op")]
    Op {
        r#ref: u64,
        conv: String,
        group: bool,
        kind: u8,
        points: Vec<f32>,
        color: String,
        text: String,
        image: Option<String>,
    },
}

pub struct Link {
    out: Sender<String>,
    inbox: Receiver<Inbound>,
    pub connected: bool,
    /// My Leyline account id and display name, once Leyline said hello back.
    pub me: Option<(String, String)>,
    pub conversations: Vec<Conversation>,
    pub last_error: Option<String>,
    next_ref: u64,
}

impl Link {
    /// Start the connection thread. `ctx` is woken whenever a message arrives.
    pub fn connect(ctx: egui::Context) -> Self {
        let (out, out_rx) = mpsc::channel::<String>();
        let (in_tx, inbox) = mpsc::channel::<Inbound>();
        std::thread::Builder::new()
            .name("leyline-link".into())
            .spawn(move || run(out_rx, in_tx, ctx))
            .expect("spawn leyline link thread");
        Self { out, inbox, connected: false, me: None, conversations: Vec::new(), last_error: None, next_ref: 1 }
    }

    pub fn send(&self, msg: Outbound) {
        if let Ok(line) = serde_json::to_string(&msg) {
            let _ = self.out.send(line);
        }
    }

    /// A fresh request id for an op whose acknowledgment we want to match.
    pub fn next_ref(&mut self) -> u64 {
        let r = self.next_ref;
        self.next_ref += 1;
        r
    }

    /// Everything that arrived since the last call. Connection state and the
    /// hello / conversation-list replies are folded into `self` on the way.
    pub fn drain(&mut self) -> Vec<Inbound> {
        let mut out = Vec::new();
        while let Ok(m) = self.inbox.try_recv() {
            match &m {
                Inbound::Connected => {
                    self.connected = true;
                    self.last_error = None;
                }
                Inbound::Disconnected => {
                    self.connected = false;
                    self.me = None;
                }
                Inbound::Me { id, name } => self.me = Some((id.clone(), name.clone())),
                Inbound::List { items } => self.conversations = items.clone(),
                Inbound::Error { msg, .. } => self.last_error = Some(msg.clone()),
                _ => {}
            }
            out.push(m);
        }
        out
    }
}

fn port() -> u16 {
    std::env::var("QSKETCH_LEYLINE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

fn run(out_rx: Receiver<String>, in_tx: Sender<Inbound>, ctx: egui::Context) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    loop {
        let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else {
            // Leyline isn't up (or isn't this version). Try again shortly; the
            // UI shows "connecting" meanwhile. Drop anything queued while we
            // were down that is only meaningful live (joins are re-sent by the
            // session tick on reconnect, and ops must not be replayed stale).
            while out_rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        let _ = stream.set_nodelay(true);
        let alive = Arc::new(AtomicBool::new(true));
        let _ = in_tx.send(Inbound::Connected);
        ctx.request_repaint();

        let reader = {
            let Ok(rd) = stream.try_clone() else { continue };
            let in_tx = in_tx.clone();
            let alive = alive.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut rd = BufReader::with_capacity(1 << 20, rd);
                let mut line = String::new();
                loop {
                    line.clear();
                    match rd.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if let Ok(m) = serde_json::from_str::<Inbound>(line.trim_end()) {
                        if in_tx.send(m).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
                alive.store(false, Ordering::Relaxed);
            })
        };

        let mut wr = stream;
        let hello = [Outbound::Hello {}, Outbound::List {}];
        let mut ok = hello.iter().all(|m| write_line(&mut wr, &serde_json::to_string(m).unwrap_or_default()));
        while ok && alive.load(Ordering::Relaxed) {
            match out_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => ok = write_line(&mut wr, &line),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let _ = wr.shutdown(std::net::Shutdown::Both);
        let _ = reader.join();
        let _ = in_tx.send(Inbound::Disconnected);
        ctx.request_repaint();
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn write_line(wr: &mut TcpStream, line: &str) -> bool {
    wr.write_all(line.as_bytes()).is_ok() && wr.write_all(b"\n").is_ok()
}

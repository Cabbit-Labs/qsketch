//! Single-instance guard: a second launch forwards its file arguments to the
//! running instance and exits, so files opened from a file manager land in a
//! new tab instead of a new window.
//!
//! Stdlib only, so it works the same on Linux and Windows: the running
//! instance listens on a loopback TCP port and records `port` + a random
//! token in `<config dir>/instance.lock`. A newcomer connects, sends the
//! token and one absolute path per line, and reads a one-byte ack. A stale
//! lock (no listener, wrong token) is simply overwritten by the newcomer.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::settings::Settings;

const LOCK_FILE: &str = "instance.lock";
/// Set on the process the updater relaunches: the old instance is on its way
/// out, so never try to hand files to it.
pub const NO_FORWARD_ENV: &str = "QSKETCH_NO_FORWARD";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Listener owned by the primary instance. Received paths are delivered on
/// `rx`; an empty batch means "just raise the window".
pub struct Primary {
    pub rx: Receiver<Vec<PathBuf>>,
    lock_path: Option<PathBuf>,
}

impl Drop for Primary {
    fn drop(&mut self) {
        if let Some(p) = &self.lock_path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Outcome of the guard at startup.
pub enum Outcome {
    /// We are the only instance: keep running and poll `Primary::rx`.
    Primary(Primary),
    /// Files were handed to an already-running instance; exit now.
    Forwarded,
}

/// Try to forward `files` to a running instance; otherwise become the primary.
/// `on_message` is invoked from the listener thread after each batch is
/// queued (used to wake the UI thread).
pub fn acquire(files: &[PathBuf], on_message: impl Fn() + Send + 'static) -> Outcome {
    let lock_path = Settings::config_dir().map(|d| d.join(LOCK_FILE));
    let skip_forward = std::env::var_os(NO_FORWARD_ENV).is_some();
    if let Some(lp) = lock_path.as_ref().filter(|_| !skip_forward) {
        if let Some((port, token)) = read_lock(lp) {
            if forward(port, &token, files) {
                return Outcome::Forwarded;
            }
        }
    }
    let (tx, rx) = mpsc::channel();
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("single-instance: bind failed ({e}); running without guard");
            return Outcome::Primary(Primary { rx, lock_path: None });
        }
    };
    let token = new_token();
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let written = match &lock_path {
        Some(lp) => write_lock(lp, port, &token),
        None => false,
    };
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(paths) = handle_client(stream, &token) {
                    if tx.send(paths).is_err() {
                        break;
                    }
                    on_message();
                }
            }
        })
        .ok();
    Outcome::Primary(Primary { rx, lock_path: if written { lock_path } else { None } })
}

fn handle_client(mut stream: TcpStream, token: &str) -> Option<Vec<PathBuf>> {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    if first.trim_end() != token {
        return None;
    }
    let mut paths = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if line.is_empty() {
            continue;
        }
        paths.push(PathBuf::from(line));
    }
    let _ = stream.write_all(b"\x06");
    Some(paths)
}

fn forward(port: u16, token: &str, files: &[PathBuf]) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else { return false };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut msg = format!("{token}\n");
    for f in files {
        let abs = std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
        let s = abs.to_string_lossy();
        if s.contains('\n') {
            continue;
        }
        msg.push_str(&s);
        msg.push('\n');
    }
    if stream.write_all(msg.as_bytes()).is_err() || stream.shutdown(std::net::Shutdown::Write).is_err() {
        return false;
    }
    let mut ack = [0u8; 1];
    matches!(stream.read(&mut ack), Ok(1) if ack[0] == 0x06)
}

fn read_lock(path: &std::path::Path) -> Option<(u16, String)> {
    let s = std::fs::read_to_string(path).ok()?;
    let mut lines = s.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_string();
    (port != 0 && !token.is_empty()).then_some((port, token))
}

fn write_lock(path: &std::path::Path, port: u16, token: &str) -> bool {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, format!("{port}\n{token}\n")).is_ok()
}

fn new_token() -> String {
    // Not a security boundary (loopback only); just enough to reject stale
    // locks whose port was reused by an unrelated process.
    let mut h = std::collections::hash_map::RandomState::new();
    use std::hash::{BuildHasher, Hasher};
    let mut a = h.build_hasher();
    a.write_u64(std::process::id() as u64);
    a.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let x = a.finish();
    h = std::collections::hash_map::RandomState::new();
    let y = h.build_hasher().finish();
    format!("{x:016x}{y:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_round_trip_and_rejects_bad_token() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = new_token();
        let t2 = token.clone();
        let server = std::thread::spawn(move || {
            let mut got = Vec::new();
            for stream in listener.incoming().flatten().take(2) {
                got.push(handle_client(stream, &t2));
            }
            got
        });
        let files = vec![PathBuf::from("/tmp/a.qsk"), PathBuf::from("/tmp/b c.png")];
        assert!(forward(port, &token, &files));
        assert!(!forward(port, "wrong", &files));
        let got = server.join().unwrap();
        assert_eq!(got[0].as_ref().unwrap(), &files);
        assert!(got[1].is_none());
    }

    #[test]
    fn dead_port_is_not_forwarded() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(!forward(port, "x", &[]));
    }
}

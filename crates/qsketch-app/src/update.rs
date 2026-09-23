//! In-app self-update.
//!
//! A signed manifest (`qsketch-latest.json`) is served from a URL the user
//! configures in Preferences ▸ General ▸ Updates (release builds may carry a
//! legacy URL that is copied into existing installs' settings once, see
//! [`LEGACY_MANIFEST_URL`]). The flow is: fetch manifest → compare versions
//! → download the platform artifact → verify its minisign signature against the
//! embedded public key → install (Windows: run the NSIS installer silently and
//! relaunch; Linux: swap the running executable from the tarball and relaunch).
//!
//! Every network step runs on a worker thread and reports back through a
//! channel that the UI polls once per frame.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Manifest URL baked into official release builds via the
/// `QSKETCH_LEGACY_MANIFEST_URL` environment variable at compile time. It is
/// used only to migrate installs that predate the user-configurable URL
/// (their settings file exists but has no `manifest_url`); fresh installs
/// start with updates disabled until a URL is entered in Preferences.
/// Source builds without the variable have no legacy URL.
pub const LEGACY_MANIFEST_URL: Option<&str> = option_env!("QSKETCH_LEGACY_MANIFEST_URL");

/// Example shown as the hint text of the manifest URL field.
pub const EXAMPLE_MANIFEST_URL: &str = "https://example.com/qsketch/qsketch-latest.json";

/// Base64 of the minisign public-key file (comment line + key). Artifacts are
/// signed by `scripts/release.sh` with the matching private key; losing that
/// key ends auto-update for existing installs.
const UPDATER_PUBKEY_B64: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEUyRThFNUMzRjdGMThEMTkKUldRWmpmSDN3K1hvNGpOdnFEbTVaNXRwVFhDa0czRnpzb0h0WFBNYmYrWlB2dERIODdJVlU1ZHYK";

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Manifest platform key for this build.
pub fn platform_key() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else {
        "unsupported"
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlatformEntry {
    pub url: String,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub pub_date: String,
    pub platforms: std::collections::HashMap<String, PlatformEntry>,
}

/// Split release notes into display blocks for the update dialog.
///
/// Notes written as a bullet list (lines starting with `-`, `*` or `•`) keep
/// their bullets; blank-line paragraphs stay paragraphs; and one long
/// paragraph — what older releases shipped — is broken at sentence ends so
/// it doesn't render as a wall of text.
pub fn notes_blocks(notes: &str) -> Vec<String> {
    let notes = notes.trim();
    if notes.is_empty() {
        return Vec::new();
    }
    let is_bullet = |l: &str| {
        let t = l.trim_start();
        t.starts_with("- ") || t.starts_with("* ") || t.starts_with("• ")
    };
    if notes.lines().any(is_bullet) {
        // Keep bullets; a line that continues one is appended to it.
        let mut out: Vec<String> = Vec::new();
        for line in notes.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if is_bullet(t) {
                out.push(t[t.find(' ').unwrap_or(0)..].trim().to_string());
            } else if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(t);
            } else {
                out.push(t.to_string());
            }
        }
        return out;
    }
    if notes.contains("\n\n") {
        return notes
            .split("\n\n")
            .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|p| !p.is_empty())
            .collect();
    }
    split_sentences(&notes.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Break a paragraph after sentence-ending punctuation, leaving version
/// numbers (`0.30.0`), ellipses (`Levels...`) and initials intact.
fn split_sentences(p: &str) -> Vec<String> {
    let chars: Vec<char> = p.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 0..chars.len() {
        if !matches!(chars[i], '.' | '!' | '?') {
            continue;
        }
        // Must be followed by a space, and then by the start of something new.
        let Some(&next) = chars.get(i + 1) else { continue };
        if !next.is_whitespace() {
            continue;
        }
        // Not an ellipsis, and not the dot inside a number like "0.30.0".
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        if prev == '.' || prev.is_ascii_digit() {
            continue;
        }
        let Some(&after) = chars[i + 1..].iter().find(|c| !c.is_whitespace()) else { continue };
        if !(after.is_uppercase() || after.is_ascii_digit() || after == '"') {
            continue;
        }
        let s: String = chars[start..=i].iter().collect();
        let s = s.trim().to_string();
        // Very short fragments read worse on their own line.
        if s.chars().count() < 16 {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(&s);
                start = i + 1;
                continue;
            }
        }
        out.push(s);
        start = i + 1;
    }
    let tail: String = chars[start..].iter().collect();
    let tail = tail.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// A newer release the user can install.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: String,
}

/// What the worker thread reports.
#[derive(Debug)]
pub enum Event {
    UpToDate,
    Available(UpdateInfo),
    Progress { downloaded: u64, total: Option<u64> },
    Ready(PathBuf),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading { downloaded: u64, total: Option<u64> },
    Ready(PathBuf),
    Failed(String),
}

/// Updater state kept in `AppState`.
pub struct Updater {
    pub status: Status,
    pub info: Option<UpdateInfo>,
    /// The result of a check should be shown in the update dialog (manual check
    /// or a new version found by the startup check).
    pub show_dialog: bool,
    /// The check was started by the user (report "up to date" / errors loudly).
    pub manual: bool,
    /// Install and relaunch as soon as the download verifies (one-click update).
    pub auto_install: bool,
    /// A short note for the status bar (a check that could not reach the
    /// server); the app moves it into the bar and clears it.
    pub notice: Option<String>,
    /// Manifest URL of the check in flight, kept for background retries.
    retry_url: Option<String>,
    /// Background re-checks still allowed after a failed manifest fetch.
    retries_left: u32,
    /// When the next background re-check starts.
    retry_at: Option<Instant>,
    rx: Option<Receiver<Event>>,
    /// Woken by the worker so results show up without waiting for input
    /// (eframe only redraws on events).
    pub ctx: Option<egui::Context>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            info: None,
            show_dialog: false,
            manual: false,
            auto_install: false,
            notice: None,
            retry_url: None,
            retries_left: 0,
            retry_at: None,
            rx: None,
            ctx: None,
        }
    }
}

/// Background re-checks after a manifest fetch fails all its in-thread
/// retries, and the pause between them. With the ~15 s of retries inside each
/// check this keeps trying for about three minutes.
const RETRY_ROUNDS: u32 = 4;
const RETRY_PAUSE: Duration = Duration::from_secs(30);

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Downloading { .. })
    }

    /// Kick off a manifest check on a worker thread. A fetch that fails is
    /// retried in the background a few times before the failure is reported.
    pub fn check(&mut self, manifest_url: String, manual: bool) {
        if self.busy() {
            return;
        }
        self.retries_left = RETRY_ROUNDS;
        self.retry_at = None;
        self.retry_url = Some(manifest_url.clone());
        self.spawn_check(manifest_url, manual);
    }

    /// Start a scheduled background re-check when it is due. Returns how long
    /// until the next one so the app can wake itself for it.
    pub fn tick(&mut self) -> Option<Duration> {
        let at = self.retry_at?;
        let now = Instant::now();
        if now < at {
            return Some(at - now);
        }
        self.retry_at = None;
        if self.busy() {
            return None;
        }
        if let Some(url) = self.retry_url.clone() {
            let manual = self.manual;
            self.spawn_check(url, manual);
        }
        None
    }

    fn spawn_check(&mut self, manifest_url: String, manual: bool) {
        self.manual = manual;
        self.status = Status::Checking;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("qsketch-update-check".into())
            .spawn(move || {
                let ev = match check_manifest(&manifest_url) {
                    Ok(Some(info)) => Event::Available(info),
                    Ok(None) => Event::UpToDate,
                    Err(e) => Event::Failed(e),
                };
                let _ = tx.send(ev);
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            })
            .ok();
    }

    /// Download + verify the available update on a worker thread.
    pub fn download(&mut self) {
        let Some(info) = self.info.clone() else { return };
        if self.busy() {
            return;
        }
        self.status = Status::Downloading { downloaded: 0, total: None };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("qsketch-update-download".into())
            .spawn(move || {
                let ev = match download_and_verify(&info, &tx, ctx.as_ref()) {
                    Ok(p) => Event::Ready(p),
                    Err(e) => Event::Failed(e),
                };
                let _ = tx.send(ev);
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            })
            .ok();
    }

    /// Drain worker events. Returns true if anything changed (repaint).
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.as_ref() else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    changed = true;
                    match ev {
                        Event::UpToDate => {
                            self.status = Status::UpToDate;
                            self.info = None;
                            if self.manual {
                                self.show_dialog = true;
                            }
                        }
                        Event::Available(info) => {
                            self.status = Status::Available;
                            self.info = Some(info);
                            self.show_dialog = true;
                        }
                        Event::Progress { downloaded, total } => {
                            self.status = Status::Downloading { downloaded, total };
                        }
                        Event::Ready(p) => self.status = Status::Ready(p),
                        Event::Failed(e) => {
                            let checking = self.status == Status::Checking;
                            self.status = Status::Failed(e.clone());
                            if !checking || self.info.is_some() {
                                // A download that failed: the dialog offers a
                                // retry and the manual route.
                                self.show_dialog = true;
                            } else if self.retries_left > 0 {
                                // The server could not be reached: keep trying
                                // quietly, and tell a user who asked once.
                                if self.retries_left == RETRY_ROUNDS && self.manual {
                                    self.notice = Some(format!(
                                        "Couldn't reach the update server ({e}); retrying in the background"
                                    ));
                                }
                                self.retries_left -= 1;
                                self.retry_at = Some(Instant::now() + RETRY_PAUSE);
                            } else {
                                self.notice = Some(format!("Update check failed: {e}"));
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(format!("qsketch/{CURRENT_VERSION}"))
        .build()
        .into()
}

/// Fetch and parse the manifest; `Some` if it advertises a newer version for
/// this platform. Relative artifact URLs resolve against the manifest URL.
pub fn check_manifest(manifest_url: &str) -> Result<Option<UpdateInfo>, String> {
    // A relay can drop a TLS handshake now and then, and a machine that just
    // woke up can be without DNS for several seconds; retry transport errors
    // with a growing pause (1, 2, 4, 8 s: ~15 s in all) before surfacing them.
    let mut body = None;
    let mut last_err = String::new();
    for attempt in 0..5u32 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(1 << (attempt - 1)));
        }
        match agent().get(manifest_url).call() {
            Ok(mut resp) => match resp.body_mut().read_to_string() {
                Ok(b) => {
                    body = Some(b);
                    break;
                }
                Err(e) => last_err = format!("manifest body: {e}"),
            },
            Err(ureq::Error::StatusCode(code)) => return Err(format!("manifest: HTTP {code}")),
            Err(e) => last_err = format!("manifest: {e}"),
        }
    }
    let Some(body) = body else { return Err(last_err) };
    let m: Manifest = serde_json::from_str(&body).map_err(|e| format!("manifest json: {e}"))?;
    newer_entry(&m, manifest_url, CURRENT_VERSION)
}

fn newer_entry(m: &Manifest, manifest_url: &str, current: &str) -> Result<Option<UpdateInfo>, String> {
    let latest = semver::Version::parse(m.version.trim()).map_err(|e| format!("manifest version: {e}"))?;
    let cur = semver::Version::parse(current).map_err(|e| format!("own version: {e}"))?;
    if latest <= cur {
        return Ok(None);
    }
    let Some(entry) = m.platforms.get(platform_key()) else {
        return Ok(None);
    };
    Ok(Some(UpdateInfo {
        version: m.version.trim().to_string(),
        notes: m.notes.clone(),
        url: resolve_url(manifest_url, &entry.url),
        signature: entry.signature.clone(),
    }))
}

/// Resolve `rel` against the manifest URL (absolute URLs pass through).
fn resolve_url(manifest_url: &str, rel: &str) -> String {
    if rel.contains("://") {
        return rel.to_string();
    }
    let base = match manifest_url.rfind('/') {
        Some(i) => &manifest_url[..=i],
        None => manifest_url,
    };
    format!("{base}{}", rel.trim_start_matches('/'))
}

/// Download the artifact into the temp dir and verify its signature.
fn download_and_verify(info: &UpdateInfo, tx: &Sender<Event>, ctx: Option<&egui::Context>) -> Result<PathBuf, String> {
    let name = info.url.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("qsketch-update");
    let dir = std::env::temp_dir().join("qsketch-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("temp dir: {e}"))?;
    let path = dir.join(name);
    let mut resp = agent().get(&info.url).call().map_err(|e| format!("download: {e}"))?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let mut reader = resp.body_mut().with_config().limit(2 * 1024 * 1024 * 1024).reader();
    let mut file = std::fs::File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_report = 0u64;
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| format!("download read: {e}"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| format!("write: {e}"))?;
        downloaded += n as u64;
        if downloaded - last_report > 512 * 1024 {
            last_report = downloaded;
            let _ = tx.send(Event::Progress { downloaded, total });
            if let Some(c) = ctx {
                c.request_repaint();
            }
        }
    }
    drop(file);
    let data = std::fs::read(&path).map_err(|e| format!("reread: {e}"))?;
    if let Err(e) = verify(&info.signature, &data) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

/// Verify a minisign signature (base64 of the signature *file*, Tauri style)
/// over `data` with the embedded public key.
pub fn verify(sig_b64: &str, data: &[u8]) -> Result<(), String> {
    let pk_text = String::from_utf8(
        data_encoding::BASE64.decode(UPDATER_PUBKEY_B64.trim().as_bytes()).map_err(|e| format!("pubkey b64: {e}"))?,
    )
    .map_err(|e| format!("pubkey utf8: {e}"))?;
    let pk_line = pk_text.lines().rev().find(|l| !l.trim().is_empty()).ok_or("empty pubkey")?;
    let pk = minisign_verify::PublicKey::from_base64(pk_line.trim()).map_err(|e| format!("pubkey: {e}"))?;
    let sig_text = String::from_utf8(
        data_encoding::BASE64.decode(sig_b64.trim().as_bytes()).map_err(|e| format!("sig b64: {e}"))?,
    )
    .map_err(|e| format!("sig utf8: {e}"))?;
    let sig = minisign_verify::Signature::decode(&sig_text).map_err(|e| format!("sig: {e}"))?;
    pk.verify(data, &sig, false).map_err(|_| "update signature does not verify".to_string())
}

/// Install a verified artifact and relaunch, passing `reopen` as file
/// arguments so the new build comes back with the same documents. On success
/// this never returns (the process exits); errors are returned so the UI can
/// show them.
pub fn install_and_relaunch(artifact: &Path, reopen: &[PathBuf]) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        install_windows(artifact, reopen)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux(artifact, reopen)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (artifact, reopen);
        Err("self-update is not supported on this platform".into())
    }
}

#[cfg(target_os = "windows")]
fn install_windows(artifact: &Path, reopen: &[PathBuf]) -> Result<(), String> {
    // Run the NSIS installer silently, then start the installed executable.
    // The installer replaces this exe, so we must exit before it copies files;
    // `start /wait` in a detached cmd sequences that for us.
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    // cmd.exe doesn't understand `\\?\` verbatim paths.
    let strip = |p: &Path| p.display().to_string().trim_start_matches(r"\\?\").to_string();
    let installer = strip(artifact);
    let exe = strip(&exe);
    let mut script = format!(
        "set {}=1 && start \"\" /wait \"{installer}\" /S && start \"\" \"{exe}\"",
        crate::single_instance::NO_FORWARD_ENV
    );
    for f in reopen {
        let f = strip(f);
        if !f.contains('"') {
            script.push_str(&format!(" \"{f}\""));
        }
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    // `raw_arg`: std's argument quoting would escape the inner quotes as `\"`,
    // which made cmd run `\\` instead of `start` ("Windows cannot find '\\'").
    std::process::Command::new("cmd")
        .arg("/C")
        .raw_arg(&script)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map_err(|e| format!("launch installer: {e}"))?;
    std::process::exit(0);
}

#[cfg(target_os = "linux")]
fn install_linux(artifact: &Path, reopen: &[PathBuf]) -> Result<(), String> {
    // The tarball holds <pkg>/bin/qsketch; extract that binary next to the
    // running executable and rename it over (Linux allows replacing a running
    // binary), then re-exec.
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe.parent().ok_or("executable has no parent directory")?;
    let file = std::fs::File::open(artifact).map_err(|e| format!("open artifact: {e}"))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let mut found = None;
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;
        let path = entry.path().map_err(|e| format!("tar path: {e}"))?.to_path_buf();
        if path.file_name().is_some_and(|n| n == "qsketch") && path.parent().is_some_and(|p| p.ends_with("bin")) {
            let tmp = dir.join(".qsketch.update-new");
            let mut out = std::fs::File::create(&tmp)
                .map_err(|e| format!("cannot write next to the executable ({}): {e}", dir.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract: {e}"))?;
            drop(out);
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod: {e}"))?;
            found = Some(tmp);
            break;
        }
    }
    let tmp = found.ok_or("the archive does not contain bin/qsketch")?;
    std::fs::rename(&tmp, &exe).map_err(|e| format!("replace executable: {e}"))?;
    let _ = std::fs::remove_file(artifact);
    // Relaunch the new binary and leave.
    std::process::Command::new(&exe)
        .args(reopen)
        .env(crate::single_instance::NO_FORWARD_ENV, "1")
        .spawn()
        .map_err(|e| format!("relaunch: {e}"))?;
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_split_into_readable_blocks() {
        // One long paragraph (how releases before 0.31 shipped).
        let one = "qsketch 0.30.0 is here. Levels (Image > Adjustments > Levels..., Ctrl+L) \
                   has a histogram. Fixed: Dust & Scratches did nothing at its default of 24.0 levels. \
                   Save first.";
        let b = notes_blocks(one);
        assert_eq!(b.len(), 4, "{b:#?}");
        assert!(b[0].ends_with("is here."));
        assert!(b[1].contains("Levels...") && b[1].ends_with("histogram."));
        assert!(b[2].starts_with("Fixed:") && b[2].contains("24.0 levels."));
        assert_eq!(b[3], "Save first.");

        // A bullet list keeps its own structure, continuation lines joined.
        let bullets = "- Levels, with a histogram\n  and live preview\n- Fixed: Dust & Scratches\n";
        assert_eq!(notes_blocks(bullets), ["Levels, with a histogram and live preview", "Fixed: Dust & Scratches"]);

        assert!(notes_blocks("   ").is_empty());
        // A short trailing fragment joins the sentence before it.
        assert_eq!(
            notes_blocks("Something happened. OK. And then more text followed here."),
            ["Something happened. OK.", "And then more text followed here."]
        );
    }

    fn manifest(version: &str) -> Manifest {
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(
            "linux-x86_64".to_string(),
            PlatformEntry { url: "qsketch-9.9.9-linux-x86_64.tar.gz".into(), signature: "sig".into() },
        );
        platforms.insert(
            "windows-x86_64".to_string(),
            PlatformEntry { url: "https://example.invalid/qsketch-9.9.9-setup.exe".into(), signature: "sig".into() },
        );
        Manifest { version: version.into(), notes: "n".into(), pub_date: String::new(), platforms }
    }

    #[test]
    fn newer_only() {
        let url = "https://host/update/qsketch-latest.json";
        assert!(newer_entry(&manifest("0.0.1"), url, "0.1.0").unwrap().is_none());
        assert!(newer_entry(&manifest("0.1.0"), url, "0.1.0").unwrap().is_none());
        let info = newer_entry(&manifest("9.9.9"), url, "0.1.0").unwrap().unwrap();
        assert_eq!(info.version, "9.9.9");
        if platform_key() == "linux-x86_64" {
            assert_eq!(info.url, "https://host/update/qsketch-9.9.9-linux-x86_64.tar.gz");
        } else if platform_key() == "windows-x86_64" {
            assert_eq!(info.url, "https://example.invalid/qsketch-9.9.9-setup.exe");
        }
    }

    #[test]
    fn resolve() {
        assert_eq!(resolve_url("https://h/update/m.json", "a.tar.gz"), "https://h/update/a.tar.gz");
        assert_eq!(resolve_url("https://h/update/m.json", "/a.tar.gz"), "https://h/update/a.tar.gz");
        assert_eq!(resolve_url("https://h/update/m.json", "https://x/y"), "https://x/y");
    }

    #[test]
    fn bad_signature_rejected() {
        assert!(verify("bm90IGEgc2ln", b"data").is_err());
    }
}

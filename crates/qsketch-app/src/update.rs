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
use std::time::Duration;

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
            rx: None,
            ctx: None,
        }
    }
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Downloading { .. })
    }

    /// Kick off a manifest check on a worker thread.
    pub fn check(&mut self, manifest_url: String, manual: bool) {
        if self.busy() {
            return;
        }
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
                            self.status = Status::Failed(e);
                            if self.manual || self.info.is_some() {
                                self.show_dialog = true;
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
    let body = agent()
        .get(manifest_url)
        .call()
        .map_err(|e| format!("manifest: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("manifest body: {e}"))?;
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

/// Install a verified artifact and relaunch. On success this never returns
/// (the process exits); errors are returned so the UI can show them.
pub fn install_and_relaunch(artifact: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        install_windows(artifact)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux(artifact)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = artifact;
        Err("self-update is not supported on this platform".into())
    }
}

#[cfg(target_os = "windows")]
fn install_windows(artifact: &Path) -> Result<(), String> {
    // Run the NSIS installer silently, then start the installed executable.
    // The installer replaces this exe, so we must exit before it copies files;
    // `start /wait` in a detached cmd sequences that for us.
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    // cmd.exe doesn't understand `\\?\` verbatim paths.
    let strip = |p: &Path| p.display().to_string().trim_start_matches(r"\\?\").to_string();
    let installer = strip(artifact);
    let exe = strip(&exe);
    let script = format!("start \"\" /wait \"{installer}\" /S && start \"\" \"{exe}\"");
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
fn install_linux(artifact: &Path) -> Result<(), String> {
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
    std::process::Command::new(&exe).spawn().map_err(|e| format!("relaunch: {e}"))?;
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

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

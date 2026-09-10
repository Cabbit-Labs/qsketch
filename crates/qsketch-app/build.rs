//! Build script for the `qsketch` binary.
//!
//! On Windows targets this embeds the application icon, version/product metadata and a DPI
//! awareness manifest into the exe via `winresource`. It is a no-op on every other target OS.
//!
//! Cross-compiling to `x86_64-pc-windows-gnu` from Linux requires the mingw-w64 `windres`
//! binary. If it isn't installed we skip resource embedding with a `cargo:warning` instead of
//! failing the build, so `cargo check`/`cargo build` still work on a dev machine that hasn't
//! installed the mingw-w64 toolchain.

use std::env;
use std::path::Path;

fn main() {
    // Re-run whenever the icon or manifest change, on every target (cheap, and keeps things
    // correct if someone runs a plain `cargo check` before ever building for Windows).
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    println!("cargo:rerun-if-changed=qsketch.manifest");
    println!("cargo:rerun-if-env-changed=WINDRES");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return; // No-op on Linux/macOS.
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // When targeting the GNU ABI, `winresource` shells out to `windres`. Figure out ahead of
    // time which binary it would try to run (mirroring winresource's own resolution logic:
    // `$WINDRES`, else `<target-prefix>windres` when cross-compiling, else plain `windres`)
    // and check it actually exists before calling into winresource, so a missing toolchain
    // degrades to a warning rather than an opaque build failure.
    if target_env == "gnu" {
        let windres = resolve_windres_path();
        let available =
            std::process::Command::new(&windres).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);

        if !available {
            println!(
                "cargo:warning=qsketch: `{windres}` not found; skipping Windows icon/manifest \
                 embedding for this build. Install mingw-w64 (e.g. `apt install \
                 mingw-w64-tools`) or set the WINDRES env var to the windres binary to fix; the \
                 exe will otherwise build fine, just without the embedded resources."
            );
            return;
        }
    }

    if let Err(e) = embed_resources() {
        // Don't fail the whole build over resource embedding (covers e.g. an MSVC toolchain
        // on Windows CI that is missing the Windows SDK's rc.exe) -- surface it loudly instead.
        println!("cargo:warning=qsketch: failed to embed Windows resources: {e}");
    }
}

/// Mirrors `winresource::WindowsResource::new()`'s own windres path resolution so we can probe
/// for it *before* handing control to winresource (which would otherwise hard-fail `compile()`).
fn resolve_windres_path() -> String {
    if let Ok(windres) = env::var("WINDRES") {
        return windres;
    }

    let host = env::var("HOST").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    let prefix = if host != target {
        match target.as_str() {
            "x86_64-pc-windows-gnu" | "x86_64-pc-windows-msvc" => "x86_64-w64-mingw32-",
            "i686-pc-windows-gnu" | "i686-pc-windows-msvc" => "i686-w64-mingw32-",
            "i586-pc-windows-gnu" | "i586-pc-windows-msvc" => "i586-w64-mingw32-",
            _ => "",
        }
    } else {
        ""
    };
    format!("{prefix}windres")
}

fn embed_resources() -> std::io::Result<()> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon_path = Path::new(&manifest_dir).join("../../assets/icon/icon.ico");
    let manifest_path = Path::new(&manifest_dir).join("qsketch.manifest");
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());

    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon_path.to_str().expect("icon path is valid UTF-8"));
    res.set_manifest_file(manifest_path.to_str().expect("manifest path is valid UTF-8"));
    res.set("ProductName", "qsketch");
    res.set("FileDescription", "qsketch \u{2014} sketching and raster painting");
    res.set("CompanyName", "Cabbit-Labs");
    res.set("LegalCopyright", "Copyright \u{a9} 2026 Cabbit-Labs and qsketch contributors");
    // winresource seeds FileVersion/ProductVersion from CARGO_PKG_VERSION already; set them
    // explicitly too so the intent isn't implicit and it stays correct if that default ever
    // changes upstream.
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);

    res.compile()
}

# Releasing qsketch

A release is one command. It bumps the version, builds Linux and Windows,
signs the two auto-update artifacts, publishes them plus the update manifest
to your update directory, verifies what your update server serves, and
commits and tags. Installed copies of qsketch that point at your manifest URL
then pick the release up themselves.

## TL;DR

```sh
scripts/release.sh 0.2.0 "One-line release notes shown in the update dialog"
git push && git push --tags
```

Use `--no-windows` to skip the Windows cross-build (the manifest keeps a
same-version Windows entry from an earlier run, if any) and `--allow-dirty`
to release with uncommitted changes (avoid; commit first).

## Before you run it

- **Working tree clean** and everything you want in the release committed.
- **CHANGELOG.md** has an `## Unreleased` section describing the changes; the
  script renames it to `## <version> — <date>`. Add one if it is missing.
- **Version** must be `MAJOR.MINOR.PATCH` and greater than the current one in
  the root `Cargo.toml` (the workspace version is the single source of truth;
  the crates inherit it and the app reads it via `CARGO_PKG_VERSION`).
- **Release notes** are one short paragraph. They appear verbatim in the
  in-app "Update available" dialog and in the release commit message.
- **Toolchain** on this host: Rust stable with the
  `x86_64-pc-windows-gnu` target, `x86_64-w64-mingw32-gcc`/`windres`,
  `makensis`, `zip`, `cargo-tauri` (only used as a minisign signer), and
  `python3`.
- **Configuration.** The script reads these from the environment or from a
  gitignored `.env` at the repo root, and refuses to run without the first
  three:

  | Variable | Meaning |
  | --- | --- |
  | `QSKETCH_UPDATER_KEY` | the minisign private key (`cargo tauri signer generate` format) |
  | `QSKETCH_UPDATE_DIR` | a local directory that your HTTP server serves as-is |
  | `QSKETCH_UPDATE_URL` | the public base URL of that directory, without a trailing slash |
  | `QSKETCH_PUBLISH_DIR` | optional; where `publish.sh` copies manual-download files |

- **Updater key.** Its public half is embedded in
  `crates/qsketch-app/src/update.rs`; a build only accepts updates signed by
  the matching private key, so a fork that wants its own update channel must
  generate a new key pair and replace the embedded public key. Losing the
  private key ends auto-update for every installed copy, so keep it backed
  up and never commit it.
- **Manifest URL.** Builds made by the script embed
  `$QSKETCH_UPDATE_URL/qsketch-latest.json` as a legacy URL that is copied
  into the settings of installs that predate the user-configurable field.
  New installs start with update checks off; users enter the URL under
  Preferences ▸ General ▸ Updates. Source builds without the variable have
  no legacy URL at all.

## What the script does

1. **Bump.** Rewrites `version` in `Cargo.toml`, refreshes `Cargo.lock`, and
   renames the changelog's `## Unreleased` heading.
2. **Build.** Runs `scripts/build-linux.sh` (tarball) and
   `scripts/build-windows.sh` (NSIS installer + portable zip). Both also copy
   their output to `QSKETCH_PUBLISH_DIR` via `scripts/publish.sh` when that
   is set.
3. **Sign.** `cargo tauri signer sign` writes `<artifact>.sig` next to the
   tarball and the installer: base64 of a minisign signature file.
4. **Publish.** Copies the tarball and installer into the update directory,
   deletes older qsketch artifacts there, and writes `qsketch-latest.json`:

   ```json
   {
     "version": "0.2.0",
     "notes": "…",
     "pub_date": "2026-09-08T20:00:00Z",
     "platforms": {
       "linux-x86_64":   { "url": "qsketch-0.2.0-linux-x86_64.tar.gz", "signature": "…" },
       "windows-x86_64": { "url": "qsketch-0.2.0-setup.exe",           "signature": "…" }
     }
   }
   ```

   Artifact URLs are relative to the manifest, so the same files work
   through any hostname or tunnel.
5. **Verify.** Fetches `$QSKETCH_UPDATE_URL/qsketch-latest.json` and
   compares each served artifact byte-for-byte with the file in the update
   directory. A mismatch aborts; an unreachable server only warns.
6. **Commit and tag.** Commits `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md`
   as `Release <version>` and creates the annotated tag `v<version>`. Pushing
   is left to you.

## After it finishes

- `git push && git push --tags`.
- Pushing the tag triggers `.github/workflows/release.yml`, which builds the
  Windows installer, portable zip and Linux tarball on GitHub and attaches
  them to a GitHub release. The in-app updater does not use GitHub; it only
  reads your manifest.
- Running copies with your manifest URL configured check it on their next
  start (at most every six hours by default). Help ▸ Check for Updates…
  forces a check. Users can confirm the new version under Help ▸ About.

## How clients update

`crates/qsketch-app/src/update.rs` fetches the manifest, compares the
`version` with its own using semver, downloads the entry for its platform,
and verifies the minisign signature before touching anything. Windows runs
the installer silently (`/S`) from a detached shell and relaunches qsketch;
Linux extracts `bin/qsketch` from the tarball, renames it over the running
executable and relaunches. Root-owned Linux installs cannot be replaced
that way and are told to update by hand.

## If something goes wrong

- **Build fails mid-way.** Nothing has been published or committed yet; fix
  and rerun with the same version. The version bump in `Cargo.toml` is already
  applied, so rerun with `--allow-dirty` or `git checkout Cargo.toml Cargo.lock
  CHANGELOG.md` first.
- **Signature missing.** `cargo tauri signer sign` needs
  `TAURI_SIGNING_PRIVATE_KEY` (the script exports it from the key file) and
  must not also be given `-k`/`-f`; two key sources make it error.
- **Verify warns.** The update server is not serving the directory. The
  files are already in place; clients will see them once it is reachable.
- **Pull a bad release.** Rerun the script with a higher version, or edit
  `qsketch-latest.json` in the update directory back to the previous version
  and artifact names (their `.sig` files are in `dist/` of that build).

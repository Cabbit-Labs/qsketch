# Contributing to qsketch

Thanks for taking a look at qsketch. This document covers how to get set up,
how CI gates things, and where to add the three kinds of extension points
people usually reach for first: tools, actions/shortcuts and panels.

## Dev setup

You need a recent stable Rust toolchain (the workspace targets edition 2021,
`rust-version = "1.85"`).

```sh
rustup update stable
git clone https://github.com/Cabbit-Labs/qsketch.git
cd qsketch
```

On Linux you'll also need the windowing/GL dependencies `eframe`/`wgpu` build
against:

```sh
sudo apt-get install libgtk-3-dev libxkbcommon-dev libwayland-dev libx11-dev \
  libxi-dev libxcursor-dev libxrandr-dev libgl1-mesa-dev pkg-config
```

Run the app:

```sh
cargo run -p qsketch
```

The workspace has two crates:

- `crates/qsketch-core` — headless document model, raster/brush/compositor
  engine and file formats. No GUI dependencies; keep it that way.
- `crates/qsketch-app` — the `egui`/`eframe`/`wgpu` desktop application.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for how they fit together.

## Before you open a PR

```sh
cargo fmt --all
cargo clippy -p qsketch-core --all-targets -- -D warnings
cargo clippy -p qsketch --all-targets
cargo test --workspace --all-targets
```

CI (`.github/workflows/ci.yml`) runs all of the above, plus a Windows MSVC
build. A couple of things worth knowing:

- **`qsketch-core` is held to a strict bar**: its clippy job runs with
  `-D warnings`. It's the stable, headless, most-tested part of the codebase,
  so warnings there fail the build.
- **`qsketch-app` clippy warnings don't fail CI yet** (the UI crate is still
  under active development), but they're still printed in the log — please
  don't add new ones on purpose.
- **rustfmt** uses the repo's `rustfmt.toml` (`max_width = 120`,
  `use_small_heuristics = "Max"`), not the default 100-column profile. Run
  `cargo fmt --all` before committing rather than hand-wrapping lines.
- `qsketch-core` has a real unit test suite (raster tiling/copy-on-write,
  brush coverage math, compositor blending, mask ops, document ops, `.qsk`
  round-trip, the keymap parser). If you touch engine or format code, add or
  update tests alongside it.

## Branches and PRs

- Branch off `main`; there's no long-lived release branch.
- Keep PRs focused — one feature or fix per PR makes review and, if
  necessary, revert much easier.
- Write commit messages and PR descriptions that explain *why*, not just
  *what*; the diff already shows what changed.
- Make sure `cargo fmt`, `cargo clippy` (per the rules above) and
  `cargo test --workspace` all pass locally before pushing.

## Where to add things

### A new tool

Tools live in `crates/qsketch-app/src/tools/`:

1. Add a variant to `ToolKind` in `tools/mod.rs`, and give it a `label()`,
   `icon()` (a Phosphor icon constant — see `ui/phosphor.rs`), toolbar
   `group()` (tools in the same group sit together with a separator between
   groups, Photoshop-style), a `cursor()`, and note in `is_paint()` /
   `uses_brush()` if it should behave like a brush.
2. Add a matching `Action` in `actions.rs` (see below) and wire it up in both
   `Action::tool()` and `Action::for_tool()` so the tool and its shortcut stay
   in sync.
3. Implement the actual pointer-event handling in the relevant module
   (`tools/paint.rs`, `tools/select.rs`, `tools/fill.rs`, `tools/transform.rs`
   or `tools/view.rs`) and route to it from the `match` in `tools::handle()`.
   A tool session that spans multiple pointer events (a drag, a stroke) is
   modeled as a `ToolSession` variant.
4. If the tool needs an options row, add it to `ToolOptions` in `tools/mod.rs`
   and surface it in `panels/tool_options.rs`.
5. If the tool draws a live preview (a marquee, a shape outline, a gradient
   line), add a case to `tools::draw_overlay()`.

Keep the actual pixel-level logic (what a stroke or selection does to a
`DocState`) in `qsketch-core` where it's unit-testable; the `tools/` module
should mostly be event plumbing and session state.

### A new action or shortcut

Every user-invokable command — menu items and tool selection alike — is
declared once in the `actions!` macro at the top of
`crates/qsketch-app/src/actions.rs`:

```rust
MyNewThing => (Category, "My New Thing…", ["Ctrl+Alt+M"]),
```

That single line generates the `Action` enum variant, its label, its
category (for menu placement), and its **default** shortcut(s) — plural,
because e.g. Redo has two (`Ctrl+Shift+Z` and `Ctrl+Y`). Users can rebind (or
clear) any of these from Edit ▸ Preferences ▸ Keyboard Shortcuts; the default
you put here is only the starting point, stored separately from the user's
`Keymap` overrides (`Settings::shortcuts` — see `settings.rs`).

Notes:

- Pick a shortcut string `Shortcut::parse` understands (see
  `key_from_name`/`key_display_name` in `actions.rs`) and check
  `Keymap::default().conflict(...)` (or just search `SHORTCUTS.md`) before
  picking a chord — two actions silently sharing a default shortcut means
  only one of them is reachable by keyboard until a user notices and rebinds
  it.
- If the action should repeat while its key is held (steppers like zoom or
  brush size), add it to `Action::repeatable()`.
- After adding or renaming an action, regenerate
  [`docs/SHORTCUTS.md`](docs/SHORTCUTS.md) from the table in `actions.rs` so
  the two don't drift.
- The actual behavior for an action lives wherever it's dispatched from
  (mainly `app.rs`'s menu/keymap handling) — the macro only declares its
  metadata.

### A new panel

Dockable panels are a `PanelKind` variant (`workspace.rs`) plus a module
under `crates/qsketch-app/src/panels/`:

1. Add the variant to `PanelKind` and include it in `PanelKind::PANELS` if it
   should be reachable from the Window menu.
2. Give it a `static_title()` and an `icon()`.
3. Create `panels/my_panel.rs` with a `pub fn ui(ui: &mut Ui, state: &mut
   AppState)` and add `mod my_panel;` to `panels/mod.rs`.
4. Add a `PanelKind::MyPanel => panels::my_panel::ui(ui, self.state)` arm to
   `Viewer::ui()` in `workspace.rs`.
5. If it should default to a particular spot in the layout, adjust
   `Workspace::default_layout()`; otherwise it will open floating (or next to
   a sensible neighbor — see `Workspace::show_panel()`) the first time a user
   opens it.

Panels are plain `egui` immediate-mode UI functions reading/writing
`AppState`; there's no separate view-model layer, so keep panel code focused
on layout and leave document mutation to `qsketch-core` functions.

## License

By contributing, you agree your contributions are licensed under both the
MIT License and the Apache License, Version 2.0 (the same dual license as
the rest of the project — see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE)).

## Publishing builds

`scripts/build-windows.sh` and `scripts/build-linux.sh` end by running `scripts/publish.sh`, which copies the installer, portable zip and tarball from `dist/` to `QSKETCH_PUBLISH_DIR` (set it in the environment or a gitignored `.env`; skipped when unset or not mounted). Signed releases for the in-app updater are cut with `scripts/release.sh`; see [`docs/RELEASING.md`](docs/RELEASING.md).

# Roadmap

qsketch is early and the core painting/layer/selection loop is the part
that's had the most attention. This page is an honest list of what isn't
there yet, split into bigger features and smaller near-term polish, so
expectations are set correctly and so contributors can see where there's
obvious room to help. Nothing here is a promise of a delivery date.

## Not yet implemented

- **Editable text layers.** The Text tool (`tools/text.rs`) rasterizes on
  commit; there is no text layer to re-open and re-edit later.
- **Layer groups.** The layer stack (`DocState::layers`) is a flat
  `Vec<Layer>`; there's no folder/group concept, so there's no way to
  collapse, move or apply opacity/blend to a set of layers as a unit.
- **Layer masks.** Layers have a `clipped` flag (clip to the layer below's
  alpha) and `alpha_locked`, but no independent per-layer grayscale mask
  channel.
- **Adjustment layers.** Invert/Desaturate/Brightness-Contrast/Hue-Saturation
  (`ops.rs`) are destructive, one-shot pixel operations applied to a layer's
  raster; there's no non-destructive adjustment-layer stack that composites
  live and can be edited or reordered later.
- **Free transform of a selection.** Pasted/floating pixels have move and
  scale handles, but a lifted selection can only be translated; there's no
  rotate or skew handle set before dropping it — only whole-canvas/whole-layer
  flip and 90°-step rotation (`ops::flip_horizontal`, `ops::rotate_canvas`,
  etc.).
- **PSD layer groups and masks.** `.psd` files open and save with layers,
  blend modes, opacity and clipping, but groups flatten into the layer list
  on open (there are no groups in qsketch yet), masks and adjustment layers
  are dropped, and 32-bit/CMYK/Lab documents are not supported.
- **Mipmapped zoom-out.** The canvas texture (`canvas/render.rs`) has a
  single mip level; `shader.wgsl` linear-filters when zoomed below 100%
  (`view.zoom < 1.0`), which can alias/shimmer at large zoom-out ratios
  (e.g. viewing a 4K canvas at 10%) compared to a proper mip chain.
- **Animation / frames.** qsketch edits a single static raster document;
  there's no timeline, frame stack, onion-skinning or export-as-animation.
- **Plugins / scripting.** There's no extension API — adding a tool, filter
  or export format currently means a source change and a rebuild (see
  [`CONTRIBUTING.md`](../CONTRIBUTING.md)).
- **Localization.** All UI strings (menu labels, tool names, panel titles)
  are English-only; there's no translation/locale infrastructure.
- **macOS packaging.** qsketch is not currently built or packaged for macOS
  — CI (`.github/workflows/*.yml`) and the release scripts
  (`scripts/build-windows.sh`, `scripts/build-linux.sh`) only target Windows
  and Linux. `directories::ProjectDirs` does resolve a sensible macOS config
  path (see the README), so a macOS build would likely need packaging/signing
  work more than settings-layer work.

## Near-term polish

- **PSB.** Large-document `.psb` files are not read; only `.psd`.
- **History panel memory.** `History` keeps up to `undo_limit` (default 100,
  configurable 2..=2000) full `DocState` snapshots; thanks to the
  copy-on-write raster this is cheap in the common case, but a very high
  undo limit combined with edits that touch most of a large canvas each step
  (e.g. big gradient fills, whole-canvas adjustments) can still add up —
  worth profiling before raising the default.
- **macOS/Linux tablet coverage.** The dedicated `octotablet` backend targets
  Windows Ink and Wayland tablet-v2; X11 and macOS tablet users are currently
  limited to whatever pressure winit's own touch-force events expose.

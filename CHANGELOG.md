# Changelog

All notable changes to qSketch are documented in this file.

## Unreleased

- Groups get Photoshop's **Pass Through** blend mode (the default for new
  groups): members blend directly with the layers below the group, so a
  Multiply layer inside a group still darkens what is underneath. Group
  opacity fades the members' effect. Pick Normal (or any other mode) to
  composite the members together first, as before.

## 0.15.0 — 2026-09-11

- Layer groups. Select several layers in the Layers panel (Ctrl+click adds
  one, Shift+click a range) and right-click ▸ **Group Selected** (`Ctrl+G`)
  folds them into a group; **Ungroup** (`Ctrl+Shift+G`) dissolves it. Groups
  nest, collapse with the caret, and carry their own opacity, blend mode,
  visibility, lock and clipping (members composite against each other
  first). Drag a layer onto the lower half of an open group to move it
  inside; drag groups as a whole. Duplicate, delete, move, merge (`Ctrl+E`
  on a group merges it into one layer) and the layer menus all understand
  groups, `.qsk` files keep them, and `.psd` export writes the members.
- Hue/Saturation (`Ctrl+H`, also `Ctrl+U`) is now a Photoshop-style dialog
  with live preview: Hue, Saturation and Lightness sliders (−100…100), a
  Colorize mode, and a Reset button. It applies only to the selected layers
  (a group stands for its members) and, when there is one, only inside the
  selection. Filters, Brightness/Contrast, Invert, Desaturate, Clear Layer
  and the layer flips likewise now apply to every selected layer.
- Layers panel: **Add Layer** at the top of the right-click menu, a group
  button in the footer, and lock/visibility of a group dims its members.

## 0.13.0 — 2026-09-11

- Free Transform (`Ctrl+T`, Edit menu): transforms the selected pixels, or
  the whole active layer when nothing is selected. Drag inside to move, the
  handles to scale (corners keep the aspect ratio, Shift frees it, Alt scales
  from the center), and outside the box to rotate (Shift snaps to 15°). A
  small panel beside the box picks the mode: **Freeform** (move / scale /
  rotate), **Resize** (move / scale only), **Rotate** (drag anywhere to spin
  about the center), **Deform** (drag the corners and edges freely,
  perspective-style; Ctrl+drag a handle in Freeform jumps straight there) and
  **Warp** (bend the pixels on a 2×2 to 6×6 control lattice). The options bar
  adds X, Y, W%, H% and angle fields, the aspect lock, Smooth/Pixel
  resampling and Reset. Enter, double-click or OK confirms; Esc or Cancel
  restores the layer. The selection follows the transformed pixels. Pasted
  images get the same transform box.
- Color panel shows either the HSV or the RGB sliders (Sliders dropdown),
  not both, so it takes up less space.
- Update check retries a dropped connection a few times before reporting
  an "Update problem".

## 0.12.6 — 2026-09-10

- Color picking is now a mouse chord that works with every tool that paints a
  color (brush, pencil, shapes, fill, gradient, text): Alt+click picks the
  foreground, Alt+right-click the background. Holding the chord's modifiers
  shows the eyedropper. Both chords are rebindable under Preferences ▸ Mouse
  (e.g. Ctrl+Shift+click or a bare right-click).

## 0.12.5 — 2026-09-10

- New default shortcuts: `R` selects the Rectangle tool, `Q` selects Rotate
  View, `Shift+W` / `Shift+S` select the layer above / below (wrapping
  around at the top and bottom of the stack).
- Ctrl+drag inside a selection moves the selected pixels with any tool; the
  pointer switches to the Move cursor while Ctrl is held over the selection.
- The Rotate View tool shows a rotate glyph as its cursor over the canvas.
- Tool options bar hides the brush settings for filled Rectangle/Ellipse
  shapes, which don't use them.
- The app version is shown at the right end of the status bar.
- Layers panel no longer overflows its dock tab: the blend/opacity header
  now fits the panel width and the layer list's scroll bar stays visible
  (the dock's own horizontal scroll area was pushing it off-screen).

## 0.12.4 — 2026-09-10

- Stylus barrel buttons mapped to right-click now work with Windows Ink on,
  with or without the dedicated tablet API setting, so pressure and
  right-click no longer have to be traded against each other. qsketch reads
  the pen button flags straight from the Windows pointer messages.

## 0.12.3 — 2026-09-10

- A stylus barrel button mapped to right-click in the tablet driver now
  works as a right-click in qsketch (quick brush popup, color pick, and so
  on) instead of starting a stroke. Requires "Use dedicated tablet API
  (Windows Ink)" in Settings → Tablet.

## 0.12.2 — 2026-09-10

- The hidden-layer eye icon is now much dimmer than the visible one on dark
  themes (and lighter on light themes), so the two states are easy to tell
  apart.
- Layer rows use a subtler alternating background.

## 0.12.1 — 2026-09-10

- Showing or hiding a layer no longer adds an undo step. Visibility is a
  view toggle: undo and redo leave it alone.
- The hidden-layer eye icon in the Layers panel is a darker grey so it
  stands out from the row.

## 0.12.0 — 2026-09-10

- qSketch now runs as a single instance. Opening a file from a file manager
  (or a second `qsketch file.qsk` launch) while it is already running opens
  the file in a new tab of the existing window and brings it to the front
  instead of starting another copy.
- Updating from inside the app now brings your work back: after the new
  build relaunches, every open file is reopened, and documents with unsaved
  changes (including untitled ones) are offered for recovery.

## 0.11.0 — 2026-09-10

- The update manifest URL is no longer built in. New installs start with
  update checks off until a URL is entered under Preferences ▸ General ▸
  Updates (Help ▸ Check for Updates… opens that page when none is set).
  Existing installs keep the URL they were updating from.
- Release tooling reads its key, update directory and URL from the
  environment or a gitignored `.env` instead of hard-coded paths.

## 0.10.1 — 2026-09-10

- New Document defaults its width and height to the image on the clipboard
  when there is one, like Photoshop.

## 0.10.0 — 2026-09-10

- Import brush tips from GIMP (`.gbr`, `.gih`) and Photoshop (`.abr`) brush
  files via the Brushes panel's Import button. Animated GIMP brushes and
  multi-brush ABR sets add one tip per brush.
- Open and save Photoshop `.psd` documents. Layer names, visibility, opacity,
  blend modes and clipping round-trip; 8- and 16-bit RGB and grayscale files
  open, and layer groups flatten into the layer list. Save keeps writing PSD
  for documents opened from one; Save As offers both formats.

## 0.9.3 — 2026-09-10

- Panel texture gains a Scale slider (0.25× to 4×) and a "Smooth filtering"
  checkbox; turn it off for crisp, aliased pixels when the tile is scaled.
- The Midnight theme is gone (too close to Graphite). Saved settings that
  used it open in Graphite.

## 0.9.2 — 2026-09-10

- Menu buttons now fill the whole title strip, so their hover highlight
  (not just the click) follows the pointer right up to the top edge.

## 0.9.1 — 2026-09-10

- Menus reach the top edge of the window: in a maximized window you can
  slam the pointer against the top of the screen and still click File,
  Edit, and the rest, just like the window buttons.
- Five more panel textures: Fading grid, Stars, Polka dots, Hearts, Argyle.
- Color panel no longer shows a scroll bar when everything fits: the picker
  now takes exactly the height left over after the sliders and recent
  colors, so a taller panel grows the picker and the whole panel stays
  visible.

## 0.9.0 — 2026-09-10

- Shortcut capture no longer binds a bare modifier press: holding Ctrl while
  choosing a chord such as Ctrl+F used to assign the shortcut to the Control
  key itself. Stored shortcuts on a bare modifier key are dropped on load.
- Duplicating the Background layer now yields a regular layer: the copy no
  longer inherits the implicit transparency lock, so the eraser reveals the
  layer beneath instead of painting the background color. Erasing on any
  layer with locked transparency shows a toast explaining why it paints.
- Layer right-click menu gains "Merge Visible" and "Flatten Image".
- Color panel fits any panel size: the picker fills the width, shrinks to a
  wide strip in a short panel, and the sliders stretch to the panel width.
- History panel no longer shows two scroll bars; its footer stays put.
- Panels and bars can carry a subtle texture: Brushed metal (default),
  Carbon weave, Paper, Fine grain, a Custom image tile, or None, with a
  strength slider and an optional top sheen (Preferences → Interface →
  Panel texture). Tiles are converted to neutral light/dark grain so they
  suit any theme.

## 0.8.1 — 2026-09-10

- Icon buttons (Tools strip, Layers/History/Swatches actions, eye/lock
  toggles, the tip button in the options bar) now light up on hover instead
  of looking greyed out.
- The Tools strip tab reads "Tools" again, next to its lock, and the strip is
  10% narrower.

## 0.8.0 — 2026-09-10

- **Compact window frame.** qSketch now draws its own title strip instead of
  the OS title bar: app icon, the File/Edit/… menus, the document title,
  an accent "Update" pill when a new version is ready, and
  minimize/maximize/close in one 24 px bar. Drag the empty part of the strip
  to move the window, double-click it to maximize, and resize from any
  window edge. Preferences › Interface › "Use system window frame" restores
  the OS decorations immediately.

## 0.7.2 — 2026-09-09

- Tool options bar simplified: the brush bar is now four groups (tool, tip,
  main knobs, and one dynamics group holding pressure, stabilizer, symmetry
  and More) instead of seven, and the accent-colored group edges and value
  bars are much fainter so the bar reads as one strip rather than a row of
  dividers.

## 0.7.1 — 2026-09-09

- Tools strip rearranging fixed: dragging no longer fights the panel's
  drag-to-scroll, tools are a uniform grid while unlocked or reordered (no
  group dividers), dropping on the left or right half of a tool places the
  dragged tool before or after it (accent insertion line shows where), and
  dropping on empty space moves it to the end.

## 0.7.0 — 2026-09-09

- **Custom theme.** Preferences › Interface has a sixth theme card, *Custom*,
  with a color picker for every slot (background, panels, widgets, border,
  text, accent, danger) and "Start from" buttons that copy a built-in theme.
  Edits apply live and are saved with your settings.
- **Icon sets.** New Preferences › Icons page with four sets: *Outline* (the
  default), *Filled*, *Classic* (more literal tool shapes) and *Classic
  Filled*. The filled sets switch every inline glyph in menus and panels too.
  An *Advanced* section lets you override any tool's icon with any of the
  1,500+ Phosphor glyphs via a searchable picker, with per-tool and global
  reset.
- **Rearrangeable Tools strip.** The Tools tab title is now a lock icon.
  Click it to unlock (it turns accent-colored), then drag tools to reorder
  them; click again to lock them in place. The order is saved; "Reset order"
  lives under Preferences › Interface. The Tools tab no longer has a close
  button (use Window › Tools to hide it).

## 0.6.1 — 2026-09-09

- Themes are monochromatic again: chips, panel tabs, the Tools strip and the
  status bar use one accent hue on neutral chrome instead of per-section
  colors.
- One-click updates: "Update and Restart" downloads, verifies, installs and
  relaunches without a second confirmation.

## 0.6.0 — 2026-09-09

- **Compact, color-coded chrome.** The menu bar, tool options bar and status
  bar are about half their previous height. Tool options are grouped into
  chips with a colored edge: blue for the tool's own settings, amber for
  brush/symmetry/stabilizer. Sliders became drag-values with a thin position
  bar (drag to change, click to type); Smoothing, Spacing, anti-aliasing and
  the stabilizer catch-up moved into a "More" menu. Dock panel tabs, the Tools
  strip group dividers, the active-tool ring and the status bar use the same
  section colors (blue tools, amber brush, pink color, green layers, purple
  view). "Compact tool options bar" is now "Minimal tool options bar".
- **Themes.** Preferences ▸ Interface offers five themes with preview cards:
  *Ink* (new default, indigo + pink from the app icon), *Graphite* (the
  previous dark look; existing "Dark" settings map to it), *Midnight* (OLED
  black), *Light* and *Sepia*.

## 0.5.0 — 2026-09-09

- **Symmetry painting.** Mirror Horizontal (`Shift+H`), Mirror Vertical
  (`Shift+V`) and radial symmetry (2–64 copies) apply to every brush-driven
  tool: Brush, Pencil, Eraser, Line, Rectangle, Ellipse and the Contour
  outline. Toggles live at the front of the options bar and under View ▸
  Symmetry; the center defaults to the canvas middle and can be placed with a
  click (crosshair button / "Set Symmetry Center…"), with guides drawn on the
  canvas. Each copy paints through its own stroke engine, so dynamics and
  scatter stay independent, and each stroke now gets a fresh random seed
  instead of repeating the same scatter pattern.
- **Stroke stabilizer.** Per-brush "Stabilizer" in the options bar: *Rope*
  (lazy brush: the stroke trails the pointer on a rope, so jitter shorter than
  the rope never lands; "Catch up" finishes at the pointer on release) and
  *Average* (moving window). Strength scales with zoom so it feels the same
  at any magnification. Saved with brush presets.
- **Crash recovery.** Every 2 minutes (Preferences ▸ General) unsaved
  documents are snapshotted to `<config>/autosave/` from a worker thread; on
  the next launch, snapshots left by a process that is no longer running are
  listed in a "Recover Unsaved Work" dialog (Recover / Discard per document or
  all). Recovered files keep their original path so Save writes back to it.
  Snapshots are deleted on save or a normal close.
- **Recent colors.** The Color panel keeps the last 20 colors painted with
  (brush, pencil, shapes, contour); click to use, right-click to set as
  background. Persisted across sessions.
- The tool options bar scrolls sideways (Shift+wheel) instead of clipping on
  narrow windows.

## 0.4.0 — 2026-09-09

- **Text tool** (`T`). Click the canvas to anchor text, type in the floating
  editor and watch it render live on the layer; Enter commits one "Text"
  history step, Shift+Enter breaks a line, Esc discards, and dragging the
  preview moves it. The options bar picks any installed font (plus egui's
  Ubuntu and Hack, and `.ttf`/`.otf` files dropped into `<config>/fonts/`),
  size, bold/italic (real faces when the family has them, synthesized
  otherwise), left/center/right alignment, line height, letter spacing and
  anti-aliasing (off for crisp pixel text). Core: `qsketch-core::text`
  (ab_glyph rasterizer), app: `fonts.rs` (fontdb discovery).
- **Contour tool** (`P`), Aseprite's filled freehand shape: drag a path, it
  closes on release and fills with the foreground color using the non-zero
  rule so self-crossing loops still fill; "Outline with brush" strokes the
  edge with the current brush in the same history step. A click is a dab.
- `Mask::from_polygon_rule` adds non-zero winding next to the even-odd lasso fill.

## 0.3.0 — 2026-09-09

- **Filter menu.** A new top-level menu between View and Window with 44
  image filters, grouped like Photoshop's: Blur (Gaussian, Box, Motion,
  Radial spin/zoom), Distort (Ripple, Wave, Twirl, Spherize, ZigZag, Polar
  Coordinates), Noise (Add Noise, Median, Dust & Scratches), Pixelate
  (Mosaic, Crystallize, Fragment, Color Halftone, Pointillize), Render
  (Clouds, Difference Clouds), Sharpen (Sharpen, Sharpen More, Unsharp
  Mask), Stylize (Find Edges, Emboss, Solarize, Diffuse, Oil Paint, Wind),
  Other (High Pass, Maximum, Minimum, Offset) and an Experimental group
  (Outline, Glow, Vignette, Pencil Sketch, Dither, Kaleidoscope, Chromatic
  Aberration, Scanlines, Pixel Sort, Glitch).
- Filter dialogs preview on the canvas while you drag; expensive filters
  wait for the slider release. Filters act on the active layer inside the
  selection, honor alpha lock, and work in premultiplied color so
  transparent edges never darken. `Ctrl+F` repeats the last filter,
  `Ctrl+Alt+F` reopens its dialog, and every filter is a remappable action.

## 0.2.4 — 2026-09-09

- **Navigator responds on press.** Pressing anywhere on the canvas preview
  centers the view there immediately, instead of waiting for a drag or a
  completed click; dragging continues to follow the pointer.

## 0.2.3 — 2026-09-09

- **Menu bar switches menus on hover.** With one menu open, moving the
  pointer over another title now opens that one (egui's built-in menu bar
  only switched on click, so the old dropdown stayed open).

## 0.2.2 — 2026-09-08

- **Fixed the brush tip angle painting mirrored.** The dial, cursor and
  preview showed the angle counter-clockwise but strokes were rotated
  clockwise; the engine now matches the dial.

## 0.2.1 — 2026-09-08

- **Fixed the Windows self-update never running the installer.** The
  `cmd /C start ... && start ...` script was passed with escaped quotes, so
  cmd tried to run `\\` ("Windows cannot find '\\'") and qSketch had already
  exited. Installs from 0.1.0/0.2.0 need this version installed by hand once.

## 0.2.0 — 2026-09-08

- **Brush customization.** New **Brush Settings** panel (**F9**, Window menu,
  the sliders button in the options bar, or double-click a preset) modeled on
  Photoshop's Brush panel: **Brush Tip Shape** (tip picker, size, flip X/Y,
  angle + roundness dial, hardness, spacing), **Shape Dynamics** (size /
  angle / roundness jitter, angle follows stroke direction or pressure,
  minimum diameter/roundness), **Scattering** (distance, both axes, count,
  pressure control), **Texture** (tiled paper grain with scale, depth,
  invert, Multiply / Subtract / Height modes, per-tip or canvas-locked),
  **Transfer** (flow jitter), **Color Dynamics** (foreground/background,
  hue, saturation, brightness jitter per dab) and **Noise**. Every section
  has an on/off box like Photoshop, values are kept while off, and a live
  stroke preview at the bottom re-renders with the real engine.
- **Custom tips and textures.** Ten procedural tips (Chalk, Charcoal,
  Spatter, Splotch, Scratchy, Bristle, Square, Star, Leaf, Flat Marker) and
  eight grain textures ship built in. Import any PNG/JPEG as a tip (dark =
  paint) or texture (light = paint), or define one from the current
  selection; user files live in `<config>/brush_tips/` and
  `<config>/textures/` and reload on start. New presets: Chalk, Charcoal,
  Spatter, Scatter Leaves, Flat Marker, Dry Bristle (existing installs: use
  the Brushes panel's restore button to see them).
- Brush preset rows now show a real rendered stroke; the canvas cursor
  follows the tip's angle and roundness; hardness hides for image tips.

- **In-app self-update.** qSketch checks a signed manifest
  (`qsketch-latest.json`) on startup (every 6 h, configurable) and via
  Help ▸ Check for Updates…, downloads the release for this platform, verifies
  its minisign signature, then installs and relaunches (Windows: silent NSIS
  installer; Linux: replaces the running executable from the tarball). Skip,
  Later, interval and manifest URL live in Preferences ▸ General ▸ Updates.
  `scripts/release.sh <version> "<notes>"` builds, signs and publishes a release.
- **Paste is now a floating placement.** Ctrl+V pastes onto the active layer
  as a floating image with handles: drag to move, drag handles to scale
  (corners keep aspect; Shift toggles), Enter/double-click applies, Esc or
  Undo discards, arrow keys nudge. Images larger than the document are
  shrunk to fit; Paste in Place keeps the original position. The paste is
  recorded as a "Paste" history step.
- **Fixed keyboard copy/cut/paste never firing.** `egui-winit` swallows the
  Ctrl+C/X/V key presses (and emits nothing for an image-only clipboard); the
  app now listens for the clipboard events and falls back to the key release.
- **Selection-aware rotate/flip.** Rotate 90° CW (**Ctrl+R**), CCW, 180° and
  Flip Horizontal (**Shift+X**) / Vertical act on the floating paste, else the
  selected pixels (in place, selection follows), else the whole canvas.
- **Rectangular Marquee is on G** (M still works); Paint Bucket moved to F.
  Export Image moved to Ctrl+Alt+Shift+S (it clashed with Merge Visible).
- **Middle-button drag pans** with any tool.
- **Right-click opens quick brush settings** (size, hardness, opacity, flow,
  smoothing, presets) for brush-based tools.
- **Alt+click picks the foreground color** and Alt+right-click the background
  with brush-based tools (previously Alt+click always set the background).
- New **Preferences ▸ Mouse** page to toggle each of the mouse behaviors above.
- The tool options bar has a fixed height and no longer jumps when a
  temporary tool (Space, Alt) is active.

## 0.1.0 — initial release

The first public release of qSketch: a fast, professional sketching and
raster painting desktop app built in Rust on `egui`/`wgpu`.

- **Raster engine.** Tiled (64x64), copy-on-write RGBA8 layer storage so undo
  snapshots and document cloning are cheap; straight-alpha layer storage with
  a premultiplied GPU compositor.
- **Brush engine.** Photoshop-style flow/opacity semantics with a per-stroke
  coverage buffer, pressure-to-size and pressure-to-opacity curves,
  configurable hardness/spacing/smoothing, and seven presets (Hard Round,
  Soft Round, Pencil, Ink Pen, Airbrush, Marker, Pixel).
- **Layers.** Opacity, 20 blend modes, alpha lock, clipping masks, visibility/
  lock toggles, reordering, duplicate, merge down, merge visible, and
  flatten.
- **Selections.** Rectangle, ellipse, lasso and magic-wand tools with add/
  subtract/intersect modifiers, anti-aliased coverage, marching-ants outline
  rendering, and move/lift/drop of floating selections.
- **Tools.** Move, rectangular/elliptical marquee, lasso, magic wand, crop,
  eyedropper, brush, pencil, eraser, paint bucket, gradient, line, rectangle,
  ellipse, zoom, hand and rotate-view.
- **Image and layer operations.** Canvas resize (9-point anchor), image
  resize (nearest/bilinear), crop, flip/rotate (canvas and per-layer), fill,
  gradient fill, invert, desaturate, brightness/contrast, hue/saturation.
- **Undo history.** A linear, jumpable history list (Photoshop-style History
  panel) backed by cheap document snapshots, with a configurable retention
  limit.
- **GPU-accelerated canvas.** A `wgpu` renderer that re-uploads only dirty
  64x64 tiles per frame, pan/zoom/rotate/flip view controls, a checkerboard
  background, and an optional pixel grid at high zoom.
- **Tablet input.** Native pen/touch pressure out of the box, plus an
  optional `octotablet` backend (Windows Ink RealTimeStylus / Wayland
  tablet-v2) for tilt and eraser-tip detection.
- **Dockable UI.** A drag/split/float workspace (`egui_dock`) with Tools,
  Layers, History, Color, Swatches, Navigator, Brushes and Info panels; the
  layout persists between sessions.
- **Fully remappable keyboard shortcuts** via Edit ▸ Preferences ▸ Keyboard
  Shortcuts, with conflict detection and per-user overrides.
- **Native `.qsk` file format.** A ZIP container of straight-alpha layer
  PNGs plus a JSON manifest, an optional selection mask and a flattened
  preview — inspectable and recoverable with ordinary tools.
- **Import/export.** PNG, JPEG, BMP, TGA, WebP, GIF and TIFF via the `image`
  crate, in addition to native `.qsk`.
- **Cross-platform packaging.** A Windows NSIS installer and portable ZIP,
  and a Linux tarball with a freedesktop.org `.desktop` entry and icons.

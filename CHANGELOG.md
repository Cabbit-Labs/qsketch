# Changelog

All notable changes to qSketch are documented in this file.

## 0.27.1 — 2026-09-20

- Canvas auto-scroll is slower and precise: it starts at a crawl and builds
  up to full speed over ~0.8 s of holding at the edge, the inner band responds
  quadratically (a light touch barely moves, the last few points before the
  edge go full speed), it eases down as the zoom climbs, and the top speed is
  a slider under Preferences ▸ Canvas (default 240 pt/s, was a fixed 900).

## 0.27.0 — 2026-09-20

- Fixed: Ctrl+= / Ctrl+- also scaled the whole UI (egui's built-in zoom) on
  top of zooming the canvas. Only the canvas zooms now; the UI scale stays
  under Preferences ▸ Interface.
- Fixed: the Polygonal Lasso only ever took its first vertex. The canvas
  dropped every press while a tool session was open, and the polygon's session
  stays open between clicks; clicks now reach the lasso until it closes.
  Holding Ctrl snaps the rubber band back to the first vertex and Ctrl+click
  closes the loop, alongside Enter, double-click and clicking the first vertex.
- Selection Brush: a new tool in the selection group that paints selection
  the way SAI's SelPen does. Strokes add to the selection; the Deselect toggle
  in the options bar (or holding Alt) makes them take away, like SelErs. It
  has its own brush (size, hardness, opacity, flow, pressure, stabilizer,
  symmetry, tip shape via Brush Settings), the selection updates live under
  the stroke, and each stroke is one undo step. No default shortcut: bind one
  under Preferences ▸ Keyboard Shortcuts.
- Canvas: auto-scroll at the edges. Dragging a marquee, lasso, shape, transform box or a
  Move-tool drag against (or past) the edge of the view scrolls the canvas in
  that direction, so a selection can grow past what is on screen when zoomed
  in. Ramps up over the last 28 points before the edge; brush strokes never
  scroll. Preferences ▸ Canvas ▸ "Auto-scroll at edges" turns it off.

## 0.26.3 — 2026-09-19

- Shortcuts: the Move tool now defaults to `A` (was `C`) and the Ellipse tool
  to `C` (was `Alt+U`). Existing custom bindings are untouched; use Reset in
  Keyboard Shortcuts to pick up the new defaults.

## 0.26.2 — 2026-09-19

- Transform: the pointer turns into a curved rotate arrow wherever a drag
  would rotate — the band just outside a Freeform transform box or the
  selection box (off the handles), anywhere in Rotate mode, and while a
  rotation is in progress — so it is clear a drag there rotates rather than
  deselects. Past the band the tool's own cursor shows, since a press there
  commits the transform and starts a fresh marquee.

## 0.26.1 — 2026-09-19

- Updater: a check that cannot reach the update server no longer opens an
  "Update problem" dialog. It retries in the background (four more rounds,
  30 s apart, each with its own ~15 s of retries, about three minutes in
  all); a check you started says so once in the status bar, and only when
  every retry fails does a short message appear there. The dialog remains for
  a download that fails, since it offers the retry and manual routes.
- Status bar: system messages show centered in the bar for eight seconds.

## 0.26.0 — 2026-09-19

- New Polygonal Lasso tool (Shift+L, next to the Lasso): click to place
  vertices, the edge to the pointer follows; click the first vertex,
  double-click or press Enter to close, Backspace removes the last vertex,
  Escape cancels. Shift / Alt add, subtract or intersect as with the other
  selection tools. A tool added in an update now appears next to its
  neighbor in a saved tool order instead of at the end of the strip.

## 0.25.6 — 2026-09-19

- Selection tools: Shift (add), Alt (subtract) and Shift+Alt (intersect) are
  read live throughout a marquee or lasso drag and at release, so letting a
  modifier go and pressing it again mid-drag, or pressing it while the pointer
  is still, works the same as holding it from the start. Shape drags read
  their constrain modifiers live the same way.

## 0.25.5 — 2026-09-19

- Quick rotate (hold Q and move the pen) works again with the pen up; 0.25.4's
  lifted-pen guard now applies only to paint strokes.

## 0.25.4 — 2026-09-19

- Lifting the pen no longer stamps a full-size dot at the end of a tapered
  stroke. Windows keeps sending mouse moves for the pen after the tip is up,
  and once the pen's pressure was gone that motion was painted at the mouse
  pressure; a stroke now ignores mouse motion while a pen in proximity is
  lifted.

## 0.25.3 — 2026-09-19

- Brush smoothing no longer cuts the corners of fast, sharp turns or stops a
  stroke short of the pointer. The filter now works on distance moved rather
  than on sample count, so the lag behind the pointer is a bounded few screen
  pixels (about 2 px at the default 20 %, 44 px at 100 %) however fast the
  hand moves or however often the mouse or pen reports, and the stroke is
  completed to where the pointer stopped on release. The smoothing length
  follows the zoom, so a zoomed-in canvas smooths over fewer document pixels.

## 0.25.2 — 2026-09-18

- Brush Settings panel: Escape closes it when it is a floating window. Docked
  in a column it stays, and a text box with focus takes the first Escape.

## 0.25.1 — 2026-09-18

- Updater: a manifest fetch that fails on the network (a DNS lookup that has
  not come back yet after waking, a dropped handshake) is retried for about
  15 seconds with a growing pause instead of 1.5 seconds, so a brief outage no
  longer ends in an "Update problem" dialog.

## 0.25.0 — 2026-09-18

- Brush Settings panel: the brush name in the header is now a text box. Enter
  (or leaving the box) renames the live brush and the preset it was loaded
  from; a name another preset already uses is refused with a note.
- Brush Settings panel: a new "Apply to preset" button (floppy icon) writes the
  settings being edited back into the preset of the same name, so a tweak can
  be kept without saving a copy. Every painting tool (brush, pencil, eraser)
  keeps its own size, hardness and other settings, as before.

## 0.24.2 — 2026-09-18

- Layers panel: right-clicking a layer row selects it (highlights it and makes
  it the active layer) before the context menu opens, instead of only once a
  menu item is picked. A right-click on a row inside a multi-selection keeps
  the selection and makes that row the active one.

## 0.24.1 — 2026-09-18

- Layers panel: the layer list's scroll bar sits on the left edge and is always
  shown as a solid, pen-sized track and handle instead of egui's thin bar that
  fades out on the right. Dragging the handle scrolls, tapping the track jumps.
- Layers panel: with the blend mode box focused, Up / Down now actually step
  through the modes; before, egui's focus navigation grabbed the keys first
  and moved focus to a neighboring control.

## 0.24.0 — 2026-09-18

- Layers panel: dragging a layer to the top or bottom edge of the list scrolls
  it, and the mouse wheel scrolls it during the drag, so a layer can be dropped
  anywhere in a long stack. The wheel also nudges the Opacity slider while the
  pointer is over it (1 % a notch, 10 % with Shift), and with the blend mode
  box focused Up / Down step through the modes without opening it.
- Color panel: the picker takes the panel's full height instead of stopping at
  a square, so a tall panel no longer leaves the bottom empty.
- Shift held with the pencil, brush or eraser previews the straight line a
  click would draw from the end of the previous stroke to the pointer. The
  pencil (and eraser) outline the exact pixels, Aseprite-style; a soft or
  anti-aliased tip shows a hairline along the path.
- Ctrl+drag inside any floating box now stamps a duplicate, not only a moved
  selection: a paste (Ctrl+V / Ctrl+Shift+V) or a Free Transform box is
  committed where it sits, its opaque pixels become the selection, and a copy
  lifts off. Ctrl on a handle still jumps into Deform.

## 0.23.1 — 2026-09-17

- 0.23.0 shipped Ungroup on Ctrl+Shift+U, which Desaturate already held;
  Ungroup is Ctrl+Alt+G.

## 0.23.0 — 2026-09-17

- View ▸ Grid: an Aseprite-style tile grid over the canvas, every 4, 8, 16,
  32, 64 or a custom number of pixels, toggled with Ctrl+Shift+G (Ungroup
  moves to Ctrl+Alt+G). It follows the view's rotation and hides once the
  cells would be smaller than 4 screen pixels. Also under Preferences ▸
  Canvas.
- The mouse wheel steps the toolbar's value bars (Size, Opacity, Flow,
  Hardness, the shape tools' Thickness…) while the pointer is over them: one
  unit for whole numbers, a hundredth of the range otherwise, a tenth of the
  value on logarithmic ranges. The wheel is taken, so nothing behind them
  scrolls.
- The Color panel is split side by side: the picker square and hue strip on
  the left, the sliders, hex field and recent colors on the right, so the
  picker gets the panel's full height.

## 0.22.0 — 2026-09-17

- The pencil is a pixel-art pencil. A hard-edged dab now lands on the pixel
  grid — centered on the pixel under the pointer for odd sizes, on the corner
  for even ones — so a 1 px pencil always fills exactly the pixel it is over.
  Zoomed in, the pointer could sit on the corner between four pixels where
  nothing was within reach: the cursor showed no pixel and a click painted
  nothing.
- The pencil cursor paints its pixels in the foreground color, at the
  stroke's opacity, so the change is on the canvas before the click; the
  outline stays. The eraser keeps to the outline.
- Hard-edged strokes step the pixel grid one pixel at a time (Bresenham), so
  a diagonal is a thin 8-connected line instead of a staircase of doubled
  corners. Shift+click lines and the Line tool do the same.
- The Eraser's tool options gain the Anti-aliasing switch the Pencil has, so
  a pixel eraser is one click away.

## 0.21.0 — 2026-09-17

- Edits still in flight can no longer be caught out by a change to the layer
  list. A paste or transform box, a text placement and a stroke each pointed
  at their layer by position and drew their preview into the working image;
  adding, duplicating, deleting, grouping, merging or reordering layers (or
  Cut, Select All, Crop, a nudge, a dialog) while one was up would aim it at
  the wrong layer — a deleted layer under a text placement was a crash — and
  snapshot the preview into history, where Esc could no longer take it back.
  Every such action now lands the pending edit first, the way filters already
  did; a History-panel jump discards it, like Undo. Layer Properties keeps its
  layer by id, so it can no longer rename a different layer.
- Hovering a compact button (Preferences ▸ "Reset all to defaults", the
  Navigator's Fit / 100% / 200%, History's Clear…) no longer nudges the
  content under it. egui's small button drops its vertical padding but still
  adds the hover outline to its frame, so it grew two pixels on hover.
- The transform box remembers the Smooth / Pixel choice across transforms
  and restarts.

## 0.20.0 — 2026-09-17

- The pencil shows the pixels it is about to paint. Instead of a circle that
  runs between pixels, the cursor outlines the exact pixel footprint of a
  press, snapped to the grid — the same coverage test the stroke rasterizes
  with, so what you see outlined is what lands. The eraser previews the same
  way, down to a single-pixel box at size 1, so the two line up when you are
  working pixel by pixel. Brushes past 256 px keep the round cursor, and the
  brush-cursor setting (Outline / Crosshair / Both / Hidden) still decides
  what is drawn.
- Switching layers while renaming one keeps the name. The edit box used to
  stay open on the old row with the typed name stranded in it; the name is now
  accepted and the box closes, on a row click or any other way the active
  layer changes. Esc still cancels.

## 0.19.0 — 2026-09-16

- Ctrl+drag duplicates as often as you like. The first Ctrl+drag lifted a copy
  of the selection, but every Ctrl+drag after that only moved the copy already
  in flight: it was captured by the transform box instead of stamping a new
  one. Ctrl+drag inside the box now drops the copy where it sits and lifts a
  fresh one from the selection it leaves behind. Ctrl on a handle still means
  Deform, and Ctrl+T keeps its own behavior.
- Moving or duplicating a selection is pixel-perfect again. Even a whole-pixel
  shift went through the resampler, where each output pixel averaged four
  bilinear taps — every move softened the pixels and feathered the edges, and
  it compounded on each repeat. A transform that only shifts the pixels by
  whole pixels now copies them verbatim.
- Delete drops the selection after clearing it, instead of leaving the
  marching ants around the hole. Both happen in one history step, so a single
  undo puts the pixels and the selection back. Preferences ▸ General ▸ "Delete
  also deselects" turns it off.

## 0.18.0 — 2026-09-16

- `C` selects the Move tool and `V` the Contour tool. Crop gives up `C` and
  ships unbound; `P`, which Contour used to hold, is free for it.
- Assigning a shortcut another action already holds now takes the key from
  that action and says so, instead of refusing the change. The old prompt
  offered Reassign / Keep both / Cancel and could read as the key simply not
  working.
- Shortcut equality ignored which spelling of Ctrl a chord carried, so a
  binding typed on the keyboard never compared equal to the same binding
  parsed from settings. Conflict detection and the "is this the default?"
  check both quietly missed because of it.
- Preferences stops reflowing while it is being used: the shortcut and chord
  buttons keep a fixed footprint as their text changes, every row's reset
  button is always drawn (disabled when there is nothing to undo) rather than
  appearing and disappearing, and the shortcut editor's messages sit on a
  reserved line instead of pushing the list down.

## 0.17.9 — 2026-09-16

- Only one mouse chord can be armed at a time. Clicking a second chord button
  left the first waiting for input too, so several rows sat on "Press a
  chord…" at once and it was anyone's guess which would take the next press.
- An armed chord button now takes the next mouse press wherever it lands,
  instead of only on the button. A chord you have to aim at the button is one
  you cannot bind when aiming is the thing that went wrong.
- Every row of Preferences ▸ Mouse has a Reset that puts that setting back to
  what it ships as, so a binding that stops working can be recovered without
  editing the settings file. Reset is greyed out when the value is already the
  default.

## 0.17.8 — 2026-09-16

- `Alt` picks a color again. Moving the pick chord to right-click left `Alt` +
  click bound to nothing, so the reflex every paint app shares stopped
  working and the status bar still advertised it. `Alt` now picks the
  foreground and `Alt` + right-click the background with any color tool,
  independent of the chords, and holding `Alt` shows the eyedropper. There is
  a toggle for it in Preferences ▸ Mouse.
- Binding a mouse chord is easier: right-click or middle-click a chord button
  to bind that button on its own, and once a button is armed a non-left press
  counts anywhere in the dialog instead of having to land on the button.
- The brush status-bar hint said right-click opened the brush settings, which
  has not been true since right-click became the color picker.

## 0.17.7 — 2026-09-16

- One cursor while picking a color. The system cursor stayed under the loupe
  alongside the precision crosshair, which showed as two pointers once the
  loupe shrank and moved up; it is hidden for as long as the loupe is up.
- The loupe sits closer to the pointer again, since it no longer needs the
  clearance its old size demanded.

## 0.17.6 — 2026-09-16

- Scaling past 10000% no longer fights itself. The width and height fields
  capped the box at 10000% and wrote that cap back on every frame, so a drag
  beyond it was undone as fast as it was made: the shape sat at the cap while
  its position kept sliding, which looked like jitter and drift. The fields
  now report only what the user types or drags into them, they ignore edits
  while a handle is being dragged, and they reach 100000%. A transform box is
  capped at 100000 pixels on a side, where the numbers stop meaning anything.

## 0.17.5 — 2026-09-16

- Thin selections transform predictably. On a selection only a few pixels
  across, the three handles along its short side pile up and a press meant
  for the corner used to land on the edge handle, scaling one axis only; a
  corner now wins over an edge handle it sits on. Corner scaling with the
  aspect ratio kept projects the pointer onto the box diagonal instead of
  taking the larger axis ratio, which let a hairline box jump to several
  hundred percent from a few pixels of drag across its thin side.
- Keep aspect is off by default for Free Transform and selection scaling;
  Shift holds the ratio while dragging a corner.

## 0.17.4 — 2026-09-16

- Transform handles stay reachable when the box is scaled past the window.
  Once a Free Transform or a selection grew beyond the visible canvas every
  handle sat off-screen, so a drag could only move the box and there was no
  way to scale it back or rotate it: the handles felt like they had stopped
  working. Handles that fall outside the view now park on its edge and drag
  the same edge or corner they always did. Nothing changes for a box that
  fits on screen.

## 0.17.3 — 2026-09-16

- The color picker loupe gained a precision crosshair on the pointer itself,
  so the pixel being sampled is marked on the canvas as well as shown in the
  magnifier. The loupe is about a third smaller, sits higher above the
  pointer, and its hex readout moved to the far side of the circle to keep the
  crosshair clear. An existing loupe size carries over to the new default
  unless it was resized by hand.

## 0.17.2 — 2026-09-16

- Right-click now picks a color on existing installs too. The new binding only
  reached fresh settings files, so an upgrade kept the old Alt+click pick chord
  and right-click went on opening the quick brush popup, doing nothing at all
  with the Rectangle and Ellipse tools since those no longer use a brush.
  Settings carry a schema number and migrate once on load; a pick chord you
  bound yourself is left alone.

## 0.17.1 — 2026-09-16

- The brush cursor follows the pointer while the quick brush settings popup is
  open, instead of staying pinned where the popup was opened, and it draws
  over the popup so a size or hardness change is visible even while the slider
  is under the pointer.

## 0.17.0 — 2026-09-16

- Pixel-perfect Rectangle and Ellipse tools. They paint hard pixels in the
  foreground color instead of stamping brush dabs, so edges are exact at any
  zoom and an ellipse is a clean pixel ellipse. The options bar gains an
  outline `Thickness` in pixels beside `Filled`, and the drag preview now
  traces the exact pixels that will be painted rather than a smooth vector
  shape. Shapes obey symmetry and the current selection as before. The Line
  tool still draws with the brush.
- Right-click picks a color. Holding it opens a round loupe of zoomed canvas
  pixels with the sampled pixel boxed at the center and its hex value below,
  so single pixels can be picked without zooming the view; the color updates
  as the pointer moves and lands on release. `Alt` + right-click picks the
  background color, the Eyedropper tool shows the same loupe, and the loupe's
  size and pixel count are in Preferences ▸ Canvas. Because a bare right-click
  is now a pick chord it takes that button from the quick brush settings
  popup, which is off by default as a result; both are rebindable in
  Preferences ▸ Mouse.
- The brush cursor previews size, hardness and roundness while the quick brush
  settings popup is open, instead of only after closing it. It is drawn at the
  point that opened the popup, stepping aside when the popup would cover it.

## 0.16.0 — 2026-09-16

- Aseprite-style selection editing. A selection now carries the eight
  transform handles: drag inside it to move the selected pixels (no Ctrl
  needed), drag a handle to scale, drag just outside the box to rotate.
  Ctrl+drag inside duplicates the selection into the current layer instead of
  moving it, leaving the original behind. Handles and the move/scale/rotate
  cursors show for the Move tool and every selection tool. A plain move no
  longer pops the Freeform/Deform/Warp panel over the canvas (Ctrl+T and
  pastes still show it), and pressing well clear of the box with a selection
  tool applies it and starts a new selection there. History records these as
  Move, Duplicate or Transform.

- Readable secondary text on any Custom theme color. Dim text (hidden layer
  names, history entries, panel tab titles, "Lock:", counts, hints) was a
  fixed gray, or egui's text faded halfway into the panel, which all but
  vanished on mid-tone chrome such as pink. It is now derived from the panel
  color with a guaranteed contrast, and every dim label uses it. Whether the
  Custom theme gets light or dark text is now decided by contrast too.

## 0.15.10 — 2026-09-14

- Layers panel scrolls to the active layer whenever it changes from
  elsewhere (Shift+W / Shift+S, undo, a canvas pick), so the highlighted row
  is always in view.

## 0.15.9 — 2026-09-14

- Chrome texture shows through list rows: the zebra stripes, hover and
  selection fills in the Layers, Brushes and History panels are translucent
  tints now instead of flat rectangles that punched holes in the texture.
- Chrome texture is visible on any theme color. The speckle pixels are now a
  lighter / darker version of the panel color rather than plain white and
  black at low alpha, which all but disappeared on mid-tone chrome such as
  a pink custom theme. Near-white chrome keeps a slightly fainter grain.

## 0.15.8 — 2026-09-14

- Exit safety net: right before the window closes, every document with
  unsaved changes is snapshotted to the crash-recovery folder, and so is any
  document whose `.qsk` on disk is missing or a different size from what is
  open, even when the app believes it is saved. The snapshots are offered
  under Recover Unsaved Work on the next start. This covers the "Confirm
  before closing unsaved work" preference being off as well.

## 0.15.7 — 2026-09-14

- Unsaved work is no longer mistaken for saved after a long session. Once the
  undo history reached its limit (200 steps) the "saved" marker compared
  equal to every later state, so the title lost its `*`, closing the window
  or the tab skipped the Unsaved Changes prompt, and crash-recovery snapshots
  stopped being written. The saved state is now tracked by a stable id that
  survives history trimming.

## 0.15.6 — 2026-09-14

- Free Transform / paste handles respond to the mouse and pen again while a
  tablet is in proximity. Tablet-sourced motion was only forwarded to paint
  sessions, so dragging a corner, rotating, deforming or moving floating
  pixels did nothing until the pointer left the tablet's range.

## 0.15.5 — 2026-09-12

- Undo/redo no longer switches the active layer: you stay on the layer you
  are on, unless that layer itself is removed by the undo.

## 0.15.4 — 2026-09-12

- Flash selected layer: when the active layer changes (Layers panel click,
  keyboard, undo), its pixels light up on the canvas for half a second and
  fade, so you can see which layer you just picked. Groups flash all their
  members. Off switch under Preferences ▸ Canvas ▸ "Flash selected layer".

- Pan inertia: a fast hand-tool drag (Space held, or middle-drag) keeps
  gliding briefly after release and eases to a stop instead of halting dead.
  Pausing before letting go, or any click, stops it. Off switch under
  Preferences ▸ Canvas ▸ "Pan inertia".

## 0.15.3 — 2026-09-11

- Pen barrel button mapped to right-click now right-clicks everywhere, not
  only on the canvas: the Layers, Brushes and Swatches context menus open
  with it. The remap happens before egui sees the input instead of inside the
  canvas.
- Pen eraser end: flipping the stylus temporarily switches to the Eraser and
  flips back when the tip returns, with any tool. On Windows the eraser is
  detected from the pointer messages directly, so it works without the
  octotablet backend and no longer depends on the driver reporting an
  inverted cursor at proximity time. Can be turned off under
  Preferences ▸ Tablet as before.
- Windows: new **WinTab** backend, on by default (Preferences ▸ Tablet ▸ "Use
  WinTab"), for Wacom tablets with "Use Windows Ink" turned off in the driver.
  Pressure, tilt, eraser end and barrel buttons come straight from the
  driver; previously the pen was a plain mouse without Ink. Falls back to the
  Windows Ink path when no driver is running. Toggling it takes effect after
  restart.

## 0.15.2 — 2026-09-11

- Quick move: hold `Ctrl` and drag with the left button to move the active
  layer (or the selected pixels, when there is a selection) with any tool,
  as the Move tool would; the Move cursor shows while the modifier is held.
  The chord is rebindable under Preferences ▸ Mouse (e.g. a Ctrl+middle or
  Alt+right drag), or can be cleared. Previously Ctrl+drag only moved with
  brush tools or from inside a selection.

## 0.15.1 — 2026-09-11

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

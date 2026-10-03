# Keyboard shortcuts

This is the full list of qsketch commands and their **default** keyboard
shortcuts, generated from the action registry in
[`crates/qsketch-app/src/actions.rs`](../crates/qsketch-app/src/actions.rs).

**Every shortcut in this list is remappable.** Open **Edit ▸ Preferences ▸
Keyboard Shortcuts** to rebind, add a second chord, or clear any of them; your
changes are saved per-user and only the bindings that differ from the
defaults below are stored. `Ctrl` reads as `Cmd` on macOS.

An action with no default shortcut is listed with an em dash and is only
reachable from its menu until you assign one.

## File

| Command | Default shortcut |
| --- | --- |
| New… | `Ctrl+N` |
| Open… | `Ctrl+O` |
| Save | `Ctrl+S` |
| Save As… | `Ctrl+Shift+S` |
| Export Image… | `Ctrl+Alt+Shift+S` |
| Export at 2× / 3× / 4× / 8×… | — |
| Export Tileset… | — |
| Export Animation… (GIF, APNG, PNG sequence, sprite sheet + JSON, MP4 / WebM via ffmpeg) | `Ctrl+Alt+Shift+A` |
| Import ▸ Frames from Files… / Sprite Sheet… | — |
| Share via Leyline… | — |
| Stop Sharing | — |
| Quick Export (re-export to the last export path, no dialog) | `Ctrl+Alt+E` |
| Timelapse ▸ Record Timelapse / Export as GIF… / Export Frames… / Clear | — |
| Close | `Ctrl+W` |
| Quit | `Ctrl+Q` |

## Edit

| Command | Default shortcut |
| --- | --- |
| Undo | `Ctrl+Z` |
| Redo | `Ctrl+Shift+Z` or `Ctrl+Y` |
| Cut | `Ctrl+X` |
| Copy | `Ctrl+C` |
| Copy Merged | `Ctrl+Shift+C` |
| Paste | `Ctrl+V` |
| Paste in Place | `Ctrl+Shift+V` |
| Clear | `Delete` |
| Fill with Foreground | `Alt+Backspace` |
| Fill with Background | `Ctrl+Backspace` |
| Free Transform | `Ctrl+T` |
| Preferences… | `Ctrl+K` |

## Image

| Command | Default shortcut |
| --- | --- |
| Image Size… | `Ctrl+Alt+I` |
| Canvas Size… | `Ctrl+Alt+C` |
| Crop to Selection | — |
| Flip Canvas Horizontal | — |
| Flip Canvas Vertical | — |
| Rotate 90° Clockwise | — |
| Rotate 90° Counter-Clockwise | — |
| Rotate 180° | — |
| Invert | `Ctrl+I` |
| Desaturate | `Ctrl+Shift+U` |
| Brightness/Contrast… | — |
| Levels… | `Ctrl+L` |
| Curves… | `Ctrl+M` |
| Liquify… | — |
| Disable/Enable Layer Mask | `Shift+Ctrl+M` |
| Color Balance… | `Ctrl+B` |
| Posterize… / Gradient Map… / Black & White | — |
| Hue/Saturation… | `Ctrl+H` (also `Ctrl+U`) |
| Replace Color… | — |
| Index Colors… | — |
| Snap to Palette… | — |
| Lock to Palette | — |

## Layer

| Command | Default shortcut |
| --- | --- |
| New Layer | `Ctrl+Shift+N` |
| Duplicate Layer | `Ctrl+J` |
| Delete Layer | — |
| Merge Down | `Ctrl+E` |
| Merge Visible | `Ctrl+Shift+E` |
| Flatten Image | — |
| Group Selected | `Ctrl+G` |
| Ungroup | `Ctrl+Shift+G` |
| Move Layer Up | `Ctrl+]` |
| Move Layer Down | `Ctrl+[` |
| Move Layer to Top | `Ctrl+Shift+]` |
| Move Layer to Bottom | `Ctrl+Shift+[` |
| Select Layer Above | `Shift+W` |
| Select Layer Below | `Shift+S` |
| Toggle Visibility | — |
| Lock Transparent Pixels | `/` |
| Lock Layer | — |
| Layer Properties… | — |
| Outline… (layer style Stroke page, switched on) | `Ctrl+Shift+O` |
| Text ▸ New Text Layer / Edit Text / Rasterize Text Layer | — |
| Shape ▸ New Shape Layer / Rasterize Shape Layer | — |
| HD Index Painting ▸ Setup / New Dither Pattern Layer | — |
| Layer Style ▸ Layer Style… / Copy / Paste / Clear Layer Style | — |
| Tilemap ▸ New Tilemap Layer / Convert to Tilemap Layer / Convert to Pixel Layer | — |
| New Adjustment Layer ▸ Brightness/Contrast / Levels / Curves / Posterize / Gradient Map / Black & White / Hue/Saturation / Color Balance | — |
| New Adjustment Layer ▸ Adjustment Layer Settings… | — |
| Flip Layer Horizontal | — |
| Flip Layer Vertical | — |
| Clear Layer | — |

## Animation

| Command | Default shortcut |
| --- | --- |
| New Frame (a copy of the current one, after it) | `Alt+N` |
| New Empty Frame | `Alt+B` |
| Duplicate Frames (the selected run) | — |
| Delete Frame(s) | `Alt+C` |
| Frame Properties… (duration, with fps presets) | `Shift+P` |
| Reverse Frames (the selected run) | — |
| Play / Stop | `Enter` |
| First / Previous / Next / Last Frame | `Home` / `,` / `.` / `End` |
| Loop Tag (playback, stepping and onion skins stay inside the current tag) | — |
| New Tag… (over the selected frames) | `Alt+T` |
| Tag Properties… / Delete Tag (the tag under the current frame) | — |
| Clear Cel / Link Cels / Unlink Cel / Copy Cel / Paste Cel / Cel Properties… | — |
| Continuous Layer (new frames link to the previous frame's cel) | — |
| Onion Skin | `F3` |

In the Timeline panel: click a cel to select its layer and frame; drag
across the frame header to scrub and select a run of frames (`Shift`+click
extends it); drag a selected run, or `Alt`+drag a frame, to move it;
`Ctrl`+wheel changes the column width; double-click a frame, tag, cel or
layer for its properties; right-click for the rest. The gear next to the
onion-skin toggle sets how many frames show, their opacity, fade and tints.

## Select

| Command | Default shortcut |
| --- | --- |
| All | `Ctrl+A` |
| Deselect | `Ctrl+D` |
| Inverse | `Ctrl+Shift+I` |
| Reselect | `Ctrl+Shift+D` |
| Feather… | `Shift+F6` |
| Expand… | — |
| Contract… | — |
| Border… | `Shift+B` (while a selection tool is active) |
| Smooth… | — |
| Sharpen | — |
| Remove Holes | — |
| Select Layer Content | `Ctrl+Alt+A` |

### Moving a selection

A selection carries eight handles. With the Move tool or any selection tool:

| Action | Mouse |
| --- | --- |
| Move the selected pixels | drag inside the selection |
| Duplicate them into the current layer | `Ctrl` + drag inside |
| Scale | drag a handle (`Shift` keeps the aspect ratio, `Alt` scales about the center) |
| Rotate | drag just outside the box (`Shift` snaps to 15°) |
| Constrain the move to one axis | `Shift` while dragging |
| Apply | `Enter`, or double-click inside the box |
| Scale a box bigger than the window | its handles park on the canvas edge |
| Cancel | `Esc` |

`Shift` and `Alt` with a selection tool still add to and subtract from the
selection, so they never grab the box. Pressing well clear of the box with a
selection tool applies it and starts a new selection there.

## View

| Command | Default shortcut |
| --- | --- |
| Zoom In | `Ctrl+=` or `Ctrl++` |
| Zoom Out | `Ctrl+-` |
| Fit on Screen | `Ctrl+0` |
| Actual Pixels | `Ctrl+1` |
| Zoom 200% | `Ctrl+2` |
| Rotate View Left | `Shift+,` |
| Rotate View Right | `Shift+.` |
| Reset View | `Escape` |
| Flip View Horizontal | `Shift+F` |
| Pixel Grid | `Ctrl+'` |
| Snap to Grid | `Ctrl+Shift+;` |
| Tiled: Off / Across / Down / Both | — |
| Fullscreen | `F11` |
| Rulers (drag off a ruler to add a guide; drag a guide back to remove it) | `Ctrl+Shift+R` |
| Guides (show / hide the document's guides) | `Ctrl+;` |
| Snap to Guides | `Ctrl+Alt+;` |
| Show Slices (outside the Slice tool) | — |
| Image ▸ Pixel Aspect Ratio ▸ Square / Double-wide / Double-tall | — |
| Lock Guides / Clear Guides | — |
| Hide/Show Panels | `Tab` |
| Arrange: Side by Side / Stacked / Grid / All in Tabs (every open document at once) | — |
| Next Document | `Ctrl+Tab` |
| Previous Document | `Ctrl+Shift+Tab` |

## Filter

Filters open a settings dialog with a live preview on the canvas (the ones
without an ellipsis apply immediately). They act on the active layer inside
the current selection and respect alpha lock. Each filter remembers its last
settings for the session.

| Command | Default shortcut |
| --- | --- |
| Last Filter (repeat with the same settings) | `Ctrl+F` |
| Last Filter Settings… | `Ctrl+Alt+F` |
| Oil Paint… | — |
| Blur ▸ Gaussian Blur…, Box Blur…, Motion Blur…, Radial Blur… | — |
| Distort ▸ Ripple…, Wave…, Twirl…, Spherize…, ZigZag…, Polar Coordinates… | — |
| Noise ▸ Add Noise…, Median…, Dust & Scratches… | — |
| Pixelate ▸ Mosaic…, Crystallize…, Fragment, Color Halftone…, Pointillize… | — |
| Render ▸ Clouds…, Difference Clouds… | — |
| Sharpen ▸ Sharpen, Sharpen More, Unsharp Mask… | — |
| Stylize ▸ Find Edges, Emboss…, Solarize, Diffuse…, Wind… | — |
| Other ▸ High Pass…, Maximum…, Minimum…, Offset… | — |
| Experimental ▸ Outline…, Glow…, Vignette…, Pencil Sketch…, Dither…, Kaleidoscope…, Chromatic Aberration…, Scanlines…, Pixel Sort…, Glitch… | — |

Every individual filter is its own action in the keymap, so any of them can
be given a shortcut from Preferences ▸ Keyboard Shortcuts.

## Tools

| Command | Default shortcut |
| --- | --- |
| Move Tool | `A` |
| Rectangular Marquee | `G` or `M` |
| Elliptical Marquee | `Shift+M` |
| Lasso | `L` |
| Polygonal Lasso | `Shift+L` |
| Magic Wand | `W` |
| Selection Brush | — |
| Crop | — |
| Slice (drag to add, drag to move, corners to resize, double-click for properties, Delete removes) | `Shift+C` |
| Tile (stamp the Tileset panel's tile; Alt or right-click clears; Ctrl+click picks) | `Shift+T` |
| Shape (click to add points to a shape layer, drag to move, Alt+click or Delete removes) | `P` |
| Eyedropper | `I` |
| Brush | `B` |
| Pencil | `N` |
| Eraser | `E` |
| Smudge | — |
| Clone Stamp | `S` |
| Paint Bucket | `F` |
| Gradient | `Shift+G` |
| Line | `U` |
| Rectangle | `R` |
| Ellipse | `C` |
| Contour | `V` |
| Text (click to start a text layer, click a text layer to edit it, drag to move) | `T` |
| Zoom | `Z` |
| Hand | `H` |
| Rotate View | `Shift+R` |
| Quick Rotate (while held; move the pointer) | `Q` |
| Reset rotation | `Q` `Q` (double-tap) |

Assigning a key that another action already uses takes it from that action:
the editor says which one lost it rather than refusing the change.

## Mouse

| Action | Default |
| --- | --- |
| Pick the foreground color (hold for a zoomed loupe and crosshair) | right-click, or `Alt` + click |
| Pick the background color | `Alt` + right-click |
| Pan the view | middle-drag |
| Move the selected layers and groups (or the selected pixels on them) with any tool | `Ctrl` + drag |

A move takes every layer selected in the Layers panel, and a selected group
takes everything inside it, hidden layers included. Locked layers and tilemaps
stay put. Pixels dragged past the canvas edge are kept outside it, so dragging
them back brings them back. Arrow keys with the Move tool nudge the same way.

`Alt` with any color tool picks a color whatever the chords say, which is the
Photoshop reflex; turn it off in Preferences ▸ Mouse if it gets in the way.
Right-click picks a color out of the box, so the quick brush settings popup
that used to own that button is off by default. Both are rebindable in
Preferences ▸ Mouse: bind the pick chords elsewhere and the popup can have
right-click back. To bind a chord, right-click or middle-click the button for
that button on its own, or left-click it and then press the chord you want
anywhere in the dialog. Each row has a Reset that restores its default. The loupe's size and pixel count live in
Preferences ▸ Canvas.

## Shapes

The Rectangle and Ellipse tools paint hard pixels in the foreground color
rather than brush dabs, so their edges are exact at any zoom. The options bar
carries `Filled` and an outline `Thickness` in pixels; the preview while
dragging traces the pixels that will be painted. The Line tool still draws
with the current brush.

While dragging a line or a gradient, `Shift` snaps the angle to 45° steps
and `Shift+Ctrl` to a finer step, 15° by default (Preferences ▸ Canvas ▸
Fine angle snap). A brush's `Shift`+click straight line goes to the click
at any angle; `Shift+Ctrl`+click snaps it to the fine step, preview
included.

## Brush

| Command | Default shortcut |
| --- | --- |
| Increase Brush Size | `]` (also Ctrl+wheel up) |
| Decrease Brush Size | `[` |
| Increase Hardness | `Shift+]` |
| Decrease Hardness | `Shift+[` |
| Swap Foreground/Background | `X` |
| Default Colors | `D` |
| Mirror Horizontal | `Shift+H` |
| Mirror Vertical | `Shift+V` |
| Symmetry Off | — |
| Set Symmetry Center… | — |
| Reset Symmetry Center (also resets the axis rotation) | — |
| Drag a symmetry guide: center moves the set, a mirror line slides, a radial line (or Alt) rotates, Shift snaps | mouse |
| Lock Symmetry Guides (clicks on the guides paint instead of grabbing them) | Alt+L |

## Window

| Command | Default shortcut |
| --- | --- |
| Tools | — |
| Layers | `F7` |
| History | — |
| Color | `F6` |
| Swatches | — |
| Palette | — |
| Navigator | — |
| Brushes | `F5` |
| Info | `F8` |
| Brush Settings | `F9` |
| Reference / Palette / Tileset | — |
| Timeline (toggles the panel) | `F10` |
| Reset Workspace | — |

## Help

| Command | Default shortcut |
| --- | --- |
| Keyboard Shortcuts… | `Ctrl+Alt+Shift+K` |
| About qsketch | — |

# The `.qsk` file format

A `.qsk` file is a plain ZIP archive (implemented in
[`crates/qsketch-core/src/io/qsk.rs`](../crates/qsketch-core/src/io/qsk.rs)).
Layer pixels are ordinary PNGs and the layer stack description is plain
JSON, so a `.qsk` stays inspectable and at least partially recoverable with
nothing but `unzip` and an image viewer, even without qsketch.

`FORMAT_VERSION` is currently `1`.

## Container layout

```
mydrawing.qsk  (a ZIP file)
├── manifest.json        # layer stack description (see below)
├── layers/
│   ├── 000.png           # bottom layer, straight-alpha RGBA8 PNG
│   ├── 000.mask.png      # optional: that layer's mask, 8-bit grayscale
│   ├── 000.outside.png   # optional (0.62+): pixels kept off the canvas
│   ├── 000.mask.outside.png  # optional (0.62+): mask coverage off the canvas
│   ├── 001.png
│   ├── 001/
│   │   ├── f0001.png     # optional (0.59+): that layer's own picture in frame 2
│   │   └── ...            # one per frame whose cel owns a non-empty picture
│   └── ...                # one file per layer, bottom to top
├── selection.png         # optional: 8-bit grayscale selection coverage
└── preview.png            # flattened straight-alpha RGBA8 PNG, for thumbnails
```

- **Layer PNGs** (`layers/NNN.png`) are stored with the `Stored` (no
  compression) ZIP method, since PNG is already compressed; `manifest.json`
  uses `Deflated`. Each is a straight-alpha RGBA8 PNG the same width/height
  as the document, one per layer, named by its position in the manifest's
  `layers` array (`layers/{i:03}.png`, zero-padded to 3 digits — practically
  unbounded, this is just for readable sorting, not a hard layer-count
  limit).
- **Layer masks** (`layers/NNN.mask.png`) are written only for layers that
  have one, named in the layer's manifest entry under `mask`. 8-bit
  grayscale the size of the document: `255` = shown, `0` = hidden. The
  entry's `mask_enabled` flag (default `true`) records a disabled mask.
- **Off-canvas pixels** (0.62+). A layer moved partly past the canvas edge
  keeps what went over. `layers/NNN.outside.png` holds those pixels (frame
  0's, for an animated layer) as one RGBA8 PNG of their bounding box, named
  in the layer's manifest entry as `"outside": { "file", "x", "y" }`, where
  `x`/`y` is the PNG's top-left in canvas coordinates (often negative).
  Pixels of that box that fall on the canvas are transparent: the layer PNG
  owns those. `mask_outside` does the same for the mask
  (`layers/NNN.mask.outside.png`, 8-bit grayscale), and a cel entry's
  `outside` for that frame's picture (`layers/NNN/fFFFF.outside.png`).
  Readers that don't know these fields open the on-canvas part.
- **Cels** (`layers/NNN/fFFFF.png`, 0.59+) hold a layer's pictures in frames
  other than the first, for documents with more than one frame.
  `layers/NNN.png` is always frame 0's picture, so a reader that knows
  nothing about frames still opens the first frame. Linked and empty cels
  write no file.
- **`selection.png`** is written only when there is a non-empty selection.
  It's an 8-bit grayscale image the same size as the document, one byte of
  coverage per pixel (`0` = unselected, `255` = fully selected,
  anti-aliased/partial selections use intermediate values). Absent means "no
  selection" on load.
- **`preview.png`** is a flattened (all visible layers composited, straight
  alpha) RGBA8 PNG of the whole document, written on every save for use as a
  recent-files thumbnail (`qsk::read_preview()` reads just this entry without
  parsing the rest of the archive).

## `manifest.json`

```jsonc
{
  "format": "qsketch",
  "version": 1,
  "app_version": "0.1.0",
  "width": 1920,
  "height": 1080,
  "active": 2,
  "next_layer_id": 5,
  "layers": [
    {
      "id": 1,
      "name": "Background",
      "visible": true,
      "locked": false,
      "alpha_locked": false,
      "opacity": 1.0,
      "blend": "Normal",
      "clipped": false,
      "file": "layers/000.png"
    }
    // ... one entry per layer, bottom to top, matching layers/*.png order
  ],
  "selection": "selection.png", // omitted (or null) if there is no selection
  "palette": { "name": "PICO-8", "colors": [{ "r": 0, "g": 0, "b": 0, "a": 255 }] }, // omitted when empty
  "palette_lock": false, // indexed-color mode: edits snap to the palette
  "stats": { "created": 1759180800, "work_secs": 5423.5, "edits": 812, "saves": 9 }, // 0.47+, optional
  "guides": [{ "vertical": true, "pos": 512.0 }, { "vertical": false, "pos": 300.0 }], // 0.49+, omitted when empty
  "timelapse": { "frames": 412, "recording": true, "every": 1 }, // 0.51+, omitted when there is none
  "pixel_aspect": [2, 1], // 0.54+, pixel width : height; omitted when square
  "slices": [{ "name": "button", "rect": { "x": 8, "y": 8, "w": 48, "h": 16 },
               "center": { "x": 4, "y": 4, "w": 40, "h": 8 }, "pivot": [24, 16],
               "color": { "r": 0, "g": 120, "b": 255, "a": 255 } }] // 0.56+, omitted when empty
}
```

Field notes:

- **`format`** must be the literal string `"qsketch"`; anything else is
  rejected on load.
- **`version`** is the container format version (`FORMAT_VERSION`, currently
  `1`). Loading a file whose `version` is *greater* than the running app's
  `FORMAT_VERSION` fails with an explicit "saved by a newer qsketch" error
  rather than silently misreading it. Loading an *older* version is expected
  to keep working as the format evolves (new fields should be added with
  `#[serde(default)]`, as `clipped` already is, so old files without them
  still parse).
- **`app_version`** is the `qsketch-core` crate version that wrote the file
  (`qsketch_core::VERSION`, i.e. the workspace version), recorded for
  diagnostics — it is not currently used to gate loading.
- **`width`** / **`height`** are the document's pixel dimensions; every layer
  PNG is expected to match them (a mismatched layer PNG, e.g. hand-edited, is
  silently placed at the origin and canvas-resized to fit rather than
  rejected).
- **`active`** is the index of the active layer at save time (clamped to a
  valid index on load, in case a hand-edited manifest is out of range).
- **`next_layer_id`** is the layer-id allocator's next value, so newly
  created layers after loading don't collide with existing layer ids. On
  load it's taken as `max(manifest value, highest existing layer id + 1)`,
  so it self-heals if the manifest's value is stale or missing.
- **`palette`** / **`palette_lock`** (0.39+) are the document palette and
  whether edits are snapped to it; both are optional and default to none /
  off, so older files load unchanged.
- **`preview.png`** (the flattened picture at the archive's root, not a
  manifest field) is what file managers show as the document's icon:
  Windows Explorer through the `qsketch-thumb` shell extension the installer
  registers, Linux file managers through the `qsketch --thumbnail` entry in
  `share/thumbnailers/qsketch.thumbnailer`. Keep writing it when extending
  the format.
- **`stats`** (0.47+) are lifetime statistics the app keeps outside the undo
  history: `created` (Unix seconds, UTC, when the document was started or
  first imported), `work_secs` (active editing time; the clock stops after
  30 s without input), `edits` (committed history steps over the document's
  life) and `saves` (including the save that wrote the file). Shown in the
  Info panel. Optional; a file without them starts fresh when opened.
- **`tilesets`** (0.57+) are the tilesets of the tilemap layers:
  `[{ "name": "Tileset 1", "tile_w": 16, "tile_h": 16, "count": 42,
  "file": "tilesets/000.png" }]`, each stored as one PNG with the tiles
  side by side, tile 0 (always empty) first.
- **`slices`** (0.56+) are named canvas rectangles (Slice tool): `rect` in
  canvas pixels, an optional 9-slice `center` and `pivot` relative to the
  rect's top-left corner, and the outline `color`. They are undoable, follow
  crops, canvas resizes, image resizes, rotations and flips, and are also
  read from and written to Aseprite slice chunks.
- **`pixel_aspect`** (0.54+) is the pixel aspect ratio (width : height,
  e.g. `[2, 1]` for double-wide pixels). It only changes how the canvas
  shows the pixels and what Export Scaled writes; layer PNGs stay 1:1.
  Aseprite files carry the same ratio in their header, and qsketch reads
  and writes it there too.
- **`timelapse`** (0.51+) describes the recorded timelapse: `frames` PNGs
  stored as `timelapse/000000.png`, `timelapse/000001.png`… (oldest first,
  each a downscaled snapshot of the composite taken after an edit),
  whether recording continues when the file is opened, and `every`, the
  number of committed edits between frames (it doubles each time the
  recording is thinned to stay under 3000 frames). Frames that are missing
  from the archive are skipped.
- **`guides`** (0.49+) are the ruler guides: `vertical` (`x = pos`) or
  horizontal (`y = pos`) lines in document pixels that the shape, marquee,
  crop, move and gradient tools snap to. Kept outside the undo history,
  like Photoshop's; omitted when there are none.
- **`layers`** is an array of per-layer properties (`LayerProps`, flattened
  into each entry) plus that layer's PNG path (`file`). Layer order in this
  array is bottom-to-top and must match the physical stacking order; there's
  no separate z-index field. Per-layer fields:
  - `id` — a stable `u64` layer identifier (not the array index; used to
    track layer identity across reorders in the app).
  - `name` — display name.
  - `visible`, `locked`, `alpha_locked` — booleans; `alpha_locked` restricts
    painting to already-opaque pixels ("lock transparent pixels").
  - `opacity` — layer opacity, `0.0..=1.0`.
  - `blend` — one of the 20 `BlendMode` variants (`Normal`, `Darken`,
    `Multiply`, `ColorBurn`, `LinearBurn`, `Lighten`, `Screen`, `ColorDodge`,
    `LinearDodge`, `Overlay`, `SoftLight`, `HardLight`, `Difference`,
    `Exclusion`, `Subtract`, `Divide`, `Hue`, `Saturation`, `Color`,
    `Luminosity`), serialized by variant name.
  - `clipped` — clips this layer to the alpha of the nearest non-clipped
    layer below it (a clipping mask). Defaults to `false` if absent, for
    forward compatibility with files written before this field existed.
  - `tilemap` (0.57+, `kind` `Tilemap`) — `{ "tileset": 0, "cols": 30,
    "rows": 17, "cells": [...] }`: row-major cell values, each a tile index
    into that tileset plus `0x80000000` (flipped horizontally) and
    `0x40000000` (flipped vertically). The layer PNG holds the rendered
    tiles, so readers that ignore tilemaps still see the right pixels.
  - `text` (0.61+, `kind` `Text`) — `{ "text": "Hello", "style": {...},
    "family": "Ubuntu", "bold": false, "italic": false, "color": [r,g,b,a],
    "anchor": [x, y] }`: the editable text the layer is drawn from. The
    layer PNG holds the rendered glyphs, so readers that ignore text layers
    still see the right pixels.
  - `continuous` (0.59+) — animation: a new frame's cel on this layer links
    to the previous frame's instead of starting empty or as a copy
    (Aseprite's "prefer linked cels"). Omitted when `false`.
  - `color` (0.59+) — an optional `[r, g, b, a]` label color for the layer
    row; `notes` (0.59+) — free text. Both are Aseprite's layer user data.
  - `cels` (0.59+) — present on pixel layers of a document with several
    frames: one entry per frame, in frame order, `{ "file": "layers/001/f0003.png" }`
    for a cel with its own picture (`"layers/001.png"` for frame 0, absent
    when the picture is empty), `{ "link": 2 }` for a cel showing frame 2's
    picture, plus optional `opacity` (`0.0..=1.0`, default `1`) and
    `z_index` (default `0`; kept for Aseprite, not drawn).
  - `kind` — `Raster` (default), `Group`, `Tilemap` (0.57+), `Shape` (0.60+), `Text` (0.61+), or (0.53+) `Adjustment`: a
    layer that owns no pixels and applies its `adjustment` (a filter such as
    `{"Levels": {...}}`, `{"Curves": {...}}`, `{"HueSaturation": {...}}`,
    `{"BrightnessContrast": {...}}` or `{"ColorBalance": {...}}`) to
    everything below it in its group, through its mask and opacity. Its
    layer PNG is empty.
  - `style` (0.52+) — layer effects, omitted when none is on:
    `drop_shadow` (`color`, `opacity`, `angle` in degrees, `distance`,
    `spread`, `size`), `outer_glow` and `inner_glow` (`color`, `opacity`,
    `spread`, `size`), `stroke` (`color`, `opacity`, `size`, `position`:
    `Outside`/`Inside`/`Center`) and `color_overlay` (`color`, `opacity`),
    each with an `enabled` flag. Effects are drawn at composite time from
    the (masked) layer pixels, never stored as pixels; `preview.png` shows
    them.
- **`selection`** is the filename of the selection PNG (currently always
  `"selection.png"` when present) or absent/`null` when there is no active
  selection.
- **`frames`** (0.59+) — `[{ "duration_ms": 100 }, ...]`, one per frame, in
  order; absent for a single-frame document. **`frame`** is the frame that
  was showing when the file was saved. **`tags`** —
  `[{ "name": "walk", "from": 0, "to": 5, "direction": "Forward" | "Reverse" |
  "PingPong" | "PingPongReverse", "repeat": 0, "color": {...} }]`; frame
  indices are zero-based and inclusive, `repeat` `0` means forever.

## Saving

`qsk::save()` writes to a `<path>.qsk.tmp` sibling file and atomically
renames it over the destination on success, so a crash or disk-full error
mid-write can't corrupt an existing save. If the document has no layers at
all (shouldn't normally happen — a `DocState` always keeps at least one), a
default `"Background"` layer is synthesized on load rather than failing to
open the file.

## Compatibility

Because layer pixels are ordinary PNGs and the manifest is human-readable
JSON, a `.qsk` file with a `version` this build doesn't understand, or a
corrupted/missing non-essential entry, can still usually be salvaged by hand
(unzip it, read `manifest.json`, open the layer PNGs directly) even if
qsketch itself refuses to load it.

## Other formats

`io::save_any` routes by extension: `.qsk` (native, lossless), `.ase` /
`.aseprite` (`io::ase`: 32-bit RGBA with every frame, linked cels and
their opacity, tags with colors as user data, groups, continuous layers,
layer colors and notes, tilesets and tilemap cels, slices, the palette and
the pixel aspect ratio), `.psd` (`io::psd`), and the flat image formats in
`io::EXPORT_EXTENSIONS` (flattened through `composite::flatten`; a `.gif`
of an animation is an animated GIF). `io::compat_warnings(path, doc)`
lists what a given target would lose, and the app shows that list before
writing. Off-canvas pixels survive both layered formats: a PSD layer
record's rect and an Aseprite cel's position may reach past the canvas, so
qsketch writes each layer's full extent there (a PSD mask that reaches off
the canvas gets its own rect with default color 0) and reads it back
outside the canvas. Only the flat image exports are canvas-sized.
`.sai2` (PaintTool SAI 2, `io::sai2`) opens but is not written. Its
layout, as qsketch reads it: a 64-byte header (`SAI-CANVAS-TYPE0`, flags,
width, height, …, chunk count at byte 32), a table of 16-byte entries
(four-letter type, object id, 64-bit offset), then the chunks. `layr`
records hold the kind, the pixel area in 32-pixel blocks (left, top,
width, height, at bytes 28-43), blend tag, opacity 0-100, flags (`0x10000`
= visible) and `name` (UTF-16); they are listed top to bottom. `lpix`
holds a layer's pixels: `dpcm`, one length per 32-pixel strip, then the
strips, each a run of 16-bit tagged records (low byte `0xff`, bits 8-11 the
block's canvas column mod 16, bits 12-15 the kind: `0` skip n+1 blocks,
`5` a block of one color as four 16-bit values, `a` a 32×32 block of n
bytes, `f` end). A block is four planes (B, G, R, A, premultiplied,
`0x4000` = full) of bit-packed differences from the gradient prediction
left + up − up-left clamped to `0..=0x4000`. `intg` is the merged image
(8-bit, 256-pixel tiles, rows filtered against the row above), used when
the layers can't be read.
`io::ase::parse` keeps every frame, cel, tag, tileset and user
data chunk of a sprite in `AseSprite`; `AseSprite::to_doc()` builds the
document with all of its frames (an animated tilemap layer becomes a pixel
layer, since qsketch tilemaps are static). `io::anim_io` writes animated
GIF (exact palette up to 255 colors), APNG, PNG sequences, sprite sheets
with Aseprite-style JSON and ffmpeg video, and reads animated GIF / APNG
and sprite sheets back into frames.

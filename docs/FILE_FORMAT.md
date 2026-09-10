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
│   ├── 001.png
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
  "selection": "selection.png" // omitted (or null) if there is no selection
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
- **`selection`** is the filename of the selection PNG (currently always
  `"selection.png"` when present) or absent/`null` when there is no active
  selection.

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

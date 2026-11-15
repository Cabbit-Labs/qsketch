# Rig export format (`qsketch-rig`, version 1)

File ▸ Export Rig for Games… (or **Export for Games…** in the Rig dialog)
writes a folder:

```
<name> rig/
  rig.json
  parts/<layer>.png      one cropped picture per visible pixel layer
```

Layers that are hidden are skipped; groups and adjustment layers are listed
in the export message and skipped. Layer masks are baked into the pictures.
Shape, text and smart-object layers are exported as their drawn pixels.

## rig.json

```jsonc
{
  "format": "qsketch-rig", "version": 1,
  "canvas": { "width": 256, "height": 256 },
  "bones": [                       // parents always come before children
    { "name": "root", "parent": null, "x": 128, "y": 200,
      "rotation": 0, "scaleX": 1, "scaleY": 1, "length": 30 }
  ],
  "parts": [                       // draw order: first = bottom
    { "name": "body", "bone": "root",   // bone is null for a static part
      "image": "parts/body.png", "width": 40, "height": 60,
      "x": 108, "y": 140,               // top-left in the setup pose (world)
      "transform": [a, b, c, d, e, f],  // see below
      "opacity": 1.0, "blend": "normal" }
  ],
  "animations": {
    "walk": {
      "duration": 1.0, "loop": true,    // seconds
      "bones": {
        "root": {
          "translate": [ { "time": 0, "x": 0, "y": 0, "curve": "linear" } ],
          "rotate":    [ { "time": 0, "angle": 0, "curve": "linear" } ],
          "scale":     [ { "time": 0, "x": 1, "y": 1, "curve": "stepped" } ]
        }
      }
    }
  }
}
```

## Coordinates

Y points down, angles are degrees and turn **clockwise** on screen.
Matrices are `[a, b, c, d, e, f]` like an SVG `matrix()`:
`x' = a·x + c·y + e`, `y' = b·x + d·y + f`.

## Posing

A bone's local transform is `translate(x, y) · rotate(rotation) · scale(scaleX, scaleY)`,
and `world(bone) = world(parent) · local(bone)` (a root's parent is the identity).

Animation values are **offsets from the setup pose**, not absolute values:

| timeline    | applied as                                   |
|-------------|----------------------------------------------|
| `translate` | `x += value.x`, `y += value.y`               |
| `rotate`    | `rotation += angle`                          |
| `scale`     | `scaleX *= value.x`, `scaleY *= value.y`     |

Sample each timeline at the playhead (clamped before the first and after
the last key). With `loop`, wrap the playhead by `duration`. A timeline with
no keys leaves its property at the setup value.

Between keys, interpolate linearly after easing the progress with the
**earlier** key's `curve`:

* `"linear"`: progress as is.
* `"stepped"`: stay at the earlier key's value until the next key.
* `[x1, y1, x2, y2]`: a cubic Bézier from (0,0) to (1,1) with those control
  points; x is the time fraction, y the progress. Solve x(s) = t for s, then
  use y(s).

## Drawing a part

For a part attached to a bone, draw its image (pixel (0,0) at the top-left)
with the matrix

```
world(bone, animated pose) · transform
```

`transform` maps image pixels into the bone's local space *as set up*, so
nothing else is needed. A part with `"bone": null` is drawn with `transform`
alone (which is just a translation to `x, y`). Multiply the part's alpha by
`opacity`.

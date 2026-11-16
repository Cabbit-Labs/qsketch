# Rig export format (`qsketch-rig`, version 1)

File ▸ Export Rig for Games… (or **Export for Games…** in the Rig dialog)
writes a folder:

```
<name> rig/
  rig.json
  parts/<layer>.png      one cropped picture per visible pixel layer
```

A part whose art differs between the document's frames gets one picture per
distinct frame (see "Frame art"). Layers that are hidden are skipped; groups and adjustment layers are listed
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
  "drives": [                      // optional; absent = none. See "Drives"
    { "bone": "small gear", "source": "big gear", "ratio": -2 }
  ],
  "ik": [                          // optional; absent = none
    { "name": "ik 1", "bones": ["thigh", "shin"], "target": "foot target",
      "bend": 1, "mix": 1 }
  ],
  "parts": [                       // draw order: first = bottom
    { "name": "body", "bone": "root",   // bone is null for a static part
      "image": "parts/body.png", "width": 40, "height": 60,
      "x": 108, "y": 140,               // top-left in the setup pose (world)
      "transform": [a, b, c, d, e, f],  // see below
      "opacity": 1.0, "blend": "normal",
      "alpha": 0.0,                     // optional: shown this much at rest
      "mesh": { "cols": 4, "rows": 3,   // optional: see "Meshes"
                "bones": ["thigh", "shin"],
                "vertices": [ [0, 1.0], [0, 0.7, 1, 0.3], ... ] } }
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
      },
      "ik": {                           // optional
        "ik 1": [ { "time": 0, "mix": 1, "bend": 1, "curve": "linear" } ]
      },
      "parts": {                        // optional: show, hide or fade a part
        "flame": [ { "time": 0.5, "alpha": 1, "curve": "stepped" } ]
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

## Drives (gears)

A drive makes one bone turn with another, like meshed gears or a belt. Each
entry of `drives` names the driven `bone`, its `source` and a `ratio`; a bone
has at most one drive and drives never form a loop.

In an animation, after sampling the timelines and before IK:

```
turned(bone) = rotate key of bone at the playhead (0 without keys)
             + ratio · turned(source)        // when the bone has a drive
rotation     = setup rotation + turned(bone)
```

So the driven bone adds `ratio` times everything its source is turned by,
including what the source's own drive adds (a train of gears), on top of its
own keys. A ratio of `-2` is a gear half the size of the one it meshes with.
Drives do nothing in the setup pose. For a loop to stay seamless the keyed
bone should end a whole number of turns from where it starts, with ratios
that keep the driven bones on whole turns too.

## Two-bone IK

Each entry of `ik` turns a two-bone chain (`bones[0]` is the parent,
`bones[1]` its child) so the child's **tip** (a point `length` along its x
axis) reaches the **head** (origin) of the `target` bone. `mix` (0 to 1)
says how much IK replaces the posed rotations; `bend` (1 or -1) picks which
way the joint folds. An animation's `ik` timeline keys `mix` (interpolated)
and `bend` (stepped: the value of the last key at or before the playhead).
Without keys the constraint's own `mix` and `bend` apply.

The setup pose is never solved: IK applies only when posing an animation.

Solve the constraints **in order**, after every bone's local transform has
been animated, working on the local rotations:

1. Compute all world matrices. Let `H` be the origin of `world(parent)`,
   `Hc` the origin of `world(child)`, `T` the origin of `world(target)`.
2. `a = |Hc − H|`, `b = child.length × |(world(child).a, world(child).b)|`,
   `d = |T − H|`. Skip the constraint if `a`, `b` or `d` is below 1e-4 or
   `mix` is 0.
3. `d = clamp(d, max(|a − b|, 1e-4), a + b)` (the chain straightens when the
   target is out of reach), `alpha = acos(clamp((a² + d² − b²) / (2·a·d), −1, 1))`.
4. `theta = atan2(T − H) + (bend < 0 ? −alpha : alpha)`.
5. `offset = atan2(Hc − H) − atan2(world(parent).b, world(parent).a)`.
   The parent's wanted world angle is `theta − offset`; its local rotation is
   that minus the world angle of the parent's own parent (0 for a root),
   in degrees: call it `rotP`.
6. `parent.rotation += mix × wrap180(rotP − parent.rotation)`, where
   `wrap180` maps degrees into −180…180.
7. Recompute the world matrices. Let `Hc` be the child's new head. The
   child's wanted world angle is `atan2(T − Hc)`; its local rotation is that
   minus the angle of `world(parent)`, in degrees: `rotC`.
   `child.rotation += mix × wrap180(rotC − child.rotation)`.

Then compute the final world matrices as usual and draw the parts.

## Meshes

A part with a `mesh` bends instead of moving as one piece. Its image is
covered by a grid of `cols × rows` cells, so `(cols + 1) × (rows + 1)`
vertices, numbered row by row: vertex `v` is column `i = v mod (cols + 1)`,
row `j = floor(v / (cols + 1))`.

* The vertex's picture position (texture coordinate) is
  `(width · i / cols, height · j / rows)` in image pixels.
* Its **setup position** is `P = (x + width · i / cols, y + height · j / rows)`
  in canvas space (`x`, `y`, `width`, `height` are the part's).
* `mesh.bones` lists bone names; `vertices[v]` is a flat list
  `[bone, weight, bone, weight, …]` where `bone` indexes `mesh.bones`. A
  vertex has at most four influences. An empty list means the vertex is not
  attached to anything and stays at `P`.

A vertex's posed position blends how far each of its bones has moved from the
setup pose. Compute every bone's setup-pose world matrix `setup(b)` once (pose
the skeleton with no animation), then

```
delta(b)  = world(b) · setup(b)⁻¹
position  = Σ weight · delta(b)(P)  /  Σ weight
```

(divide by the weight sum so weights need not add to exactly 1). Draw each
cell as two triangles, `(v00, v10, v01)` and `(v10, v11, v01)` where `v10` is
the vertex to the right of `v00` and `v01` the one below it, mapping the image
triangle with the texture coordinates onto the triangle at the posed
positions. **A meshed part ignores `bone` and `transform`**; the weights are
the whole story. Its image is exactly the mesh rectangle, so the grid always
covers it.

## Frame art (image swaps)

When the document has several frames and a layer's picture differs between
them (a hand-drawn flame flicker, mouth shapes), the file has

```jsonc
"frames": [0.09, 0.09, 0.09, 0.09],          // top level: each frame's length, seconds
"parts": [ { "name": "fire", "image": "parts/fire_f1.png",
             "images": ["parts/fire_f1.png", "parts/fire_f2.png",
                        "parts/fire_f3.png", "parts/fire_f4.png"], ... } ]
```

`images` has one entry per frame (frames that share a picture repeat the same
file). All of a part's pictures have the same size and use the part's one
`x`, `y`, `width`, `height`, `transform` and `mesh`, so only the texture
changes. The frames **cycle** for as long as an animation plays, whether or
not the animation loops. To find the frame at a playhead of `seconds`, work in
whole milliseconds: `t = round(seconds × 1000) mod (sum of the frame lengths in
ms)`, then walk the frames subtracting each length until `t` is less than the
next one. Draw `images[frame]`. A part without `images` always draws `image`,
and the setup pose draws every part's `image`.

## Showing, hiding and fading parts

A part's `alpha` (absent means 1) is how shown it is when an animation does
not key it: a flame that only appears in one animation has `"alpha": 0`. An
animation's `parts` timeline keys the alpha of a part by name. Sample it like
any other timeline (clamped before the first and after the last key, the
earlier key's `curve` between keys; `"stepped"` switches instantly, `"linear"`
fades). A part with no timeline in the playing animation uses its own `alpha`,
and so does the setup pose. When drawing, multiply the part's `opacity` by
the alpha; at 0 there is nothing to draw.

## Drawing a part

For a part without a `mesh` that is attached to a bone, draw its image (pixel (0,0) at the top-left)
with the matrix

```
world(bone, animated pose) · transform
```

`transform` maps image pixels into the bone's local space *as set up*, so
nothing else is needed. A part with `"bone": null` is drawn with `transform`
alone (which is just a translation to `x, y`). Multiply the part's alpha by
`opacity` (and by its animated alpha, see above).

## Reference player

`examples/rig-player/` has a JavaScript runtime written from this document
(`rig-runtime.js`), a canvas player, and a test that checks the runtime
against the editor's own poses. Start there when writing a runtime for
another engine.

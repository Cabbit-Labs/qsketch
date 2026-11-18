# q3d: paint textures with the 3D model beside you

**Status: steps 1, 2, 2b and 4 are in the app (since 0.69.0) and still experimental; the rest is planned.** Each
step below is marked as it lands.

A texture atlas is hard to read: the pixels for one part of a model are scattered across many islands. q3d puts the
model in a dockable qSketch panel, showing the document as its live texture, so you can see what you are painting and
say which part of the model you mean.

It came out of painting a banana peel for a game mod, whose atlas has 888 UV islands. It is built in, not a bridge to
a separate viewer, because selecting and highlighting need the canvas and the 3D view to share state on every frame.

## Principles

- **The document stays an ordinary image.** Guides, tints and labels are drawn over the canvas and are never saved
  into it, so they cannot end up flattened into a PNG.
- **The model file is not modified.** Sections live in a small sidecar file next to it.
- **Animations are read, not shown.** `qsketch-3d` keeps a file's nodes, which node each piece of the mesh hangs on, and its node
  animations (`anim.rs`: translation, rotation and scale keys; rigid, no skins), and `Mesh::posed` moves the pieces to a moment of
  one. The Model panel paints on the rest pose; Resin (`/code/resin`), which shares the crate, plays them.
- **The core is headless.** `crates/qsketch-3d` (mesh loading, UV analysis, sectioning) has no GUI or GPU code and
  is unit tested. Drawing and panels live in `qsketch-app`.

## Steps

| # | Step | Status |
|---|------|--------|
| 1 | Model panel: open a GLB, orbit it, wear a document as a live texture | Done |
| 1b | Parts and variants: show and hide named parts, one per slot | Done |
| 1c | Palette regions: keep paint to a console texture's per-page colors | Done |
| 2 | Sections: find them, list, pick on the model or the canvas, highlight in both views, rename and merge | Done; splitting a section by hand is not built |
| 2b | Rearrange Texture: one continuous island per section, with the paint baked across | Done; a few pieces still fold |
| 3 | Overlays: island outlines, tints, labels, mapped pixels only | Mapped pixels done (Dim Unused Texels); the rest planned |
| 4 | Paint limits: paint only inside a section, through the selection system | Done: Select on Canvas |
| 5 | Paint directly on the 3D model | Later |
| 6 | Simplifier: reduce a heavy mesh and bake its texture across | Later |

### 1. Model panel

- Ways in: the **Model** panel (Window menu), `qsketch model.glb`, dropping a `.glb` on the window, or File > Open. The
  panel opens docked beside the canvas, so the paint and the model can be watched together.
- The panel samples the GPU texture the canvas already keeps for each document, so strokes appear on the model as
  they are made, with no save. The document's canvas must be visible, since only visible documents upload tiles.
- The 3D view renders off screen with its own depth buffer and MSAA, because egui's render pass has neither, and is
  then copied into the panel.
- Controls: drag to orbit, scroll to zoom, middle or right-drag to slide the model, spin, texture document (the
  active one or a fixed one), and rotate 90 degrees, since generated models often come in with the wrong axis up.
- **Extract Texture** saves the color texture stored inside the model (PNG, JPEG or WebP) as `name_texture` beside
  it, never overwriting, and opens that as the texture document. The model file itself is not changed. A model with a
  texture of its own opens wearing it: `name_texture` if an earlier extraction left it (with any paint since), or the
  texture extracted then.
- An opened model joins File > Open Recent, and the empty Model panel offers to reopen the last one.
- The texture is drawn as the file asks: point sampled when its sampler's filter is `NEAREST`, and with see-through texels
  left out when the material's `alphaMode` is `MASK`. First used for a PlayStation model (Mega Man of *Mega Man Legends*,
  exported from the user's disc by the decompilation's Control Lab), whose 4-bit texture pages blur under smoothing.
- Not yet: only GLB with an embedded buffer loads. Draco and
  meshopt compressed models are refused with a message saying so.

### 1b. Parts and variants

A model whose file names its pieces (one glTF node each) lists them above the sections. Parts that share a slot (the node's
`extras.slot`) replace one another, as a game character's helmets or the weapons on one arm do: each slot is one choice, so
one is shown at a time. Other parts are checkboxes. A node with `KHR_node_visibility` `visible: false` starts hidden. A hidden
part is not drawn, cannot be clicked, and claims no texels, so a canvas pick or highlight finds what is shown. Such a model
starts with its parts as its sections rather than the sectioner's guess. **Dim Unused Texels** darkens the canvas wherever no
shown part reads the texture, so it is plain which pixels painting would change on the model as shown, and which belong to a
hidden variant or to nothing. First used for Mega Man of *Mega Man Legends* with his heads, shoes and weapon arms (19 parts in 5
slots), exported by the decompilation's Control Lab. Which parts are shown is kept in the sidecar (`shown`) and comes back
with the model. The parts list folds away under its header, the section list leaves out sections whose triangles are all
hidden, and a model opens framed on the parts shown rather than on every variant.

### 1c. Palette regions (console textures)

A console texture is indexed: a PlayStation texture page holds 4-bit texels and is drawn through one 16-color table,
so a texel can only be one of those colors or see-through. A model can say so: its base color image's
`extras.palette_regions` is `{"size": [w, h], "regions": [{"rect": [x, y, w, h], "colors": [[r, g, b, a], ...],
"name": "..."}]}`. While the model wears a document of that size and **Keep to Texture Palettes** is on, every
committed stroke snaps each changed pixel to its region's nearest color (`qsketch_core::palette::PaletteRegion`):
under half alpha to the region's see-through entry, otherwise fully opaque. The brush color is kept to the regions'
colors, the document palette (when empty) shows them all, and the panel shows the 16 of the region under the pointer
to pick from, with a warning when the brush color is not one of them, since a stroke there would take its nearest.
First used on Mega Man of *Mega Man Legends* (three regions: his body page, the same page through a second table, and
his face page); after painting, every texel of the saved PNG was still one of its region's colors.

### 2. Sections

A section is a named set of triangles, drawn from one UV island or several. A triangle's UV gives its canvas pixels,
so one selection can be shown in both views. Picking on the model tests the click against the projected triangles.
Picking on the canvas (Ctrl+Alt+right-click) looks up a map of which section owns each texture pixel; the same map,
kept until the model or its sections change, makes the canvas highlight a single image.

A model loads and has its sections found on a background thread, so even a million triangles open without
freezing the app.

### The sectioner (generated models)

Hand-made models have a few clean islands, and those can be sections as they stand. Generated models do not: the
peel has 888 islands, and many are scraps a few pixels wide. The sectioner turns that into a handful of useful parts:

1. **Connect the surface:** weld vertices that UV seams split, and link triangles across shared edges. Loose
   fragments are set aside and later join the nearest part.
2. **Find the tips:** repeatedly take the surface point farthest from every tip so far, until new ones are close to
   existing ones. Tips close together in 3D are one end seen from both faces of a thick sheet, and tips that barely
   reach from the hub are bumps, so those are merged or dropped.
3. **Assign triangles:** each goes to a tip by surface distance divided by that tip's reach from the hub, so a short
   protrusion such as a stem keeps its share.
4. **Split sides:** flat or long parts split by which way each triangle faces, measured against a plane fitted to each
   slice along the part, because a curved strip has no single plane. Neighbors then vote to clear speckle along the rims.
   With a texture available, a part painted in two clearly different colors splits along that paint line instead,
   which follows real surfaces better: facing direction put cream and yellow into the same section on the peel.
5. **Name and keep:** right-click a section to rename it or merge it into another. The result is saved in
   `model.sections.json` beside the model, together with its quarter turns, and read back when the model opens, so
   the automatic pass runs once and those edits are kept. The file is ignored if the model's triangle count has
   changed. "Discard Edits and Find Sections Again" in the same menu deletes it. Splitting a section by hand is
   not built yet.

The peel comes out as four strips with a top and underside each, plus the stem and hub: nine sections in about
0.13 s for 139,428 triangles and 1.1 s for 987,308. `cargo run -p qsketch-3d --example section_report -- model.glb -z`
prints the result for a model. Sections span islands freely, so a poor UV layout does not limit them. A triangle-level split is allowed where one
island crosses parts, which is common on these models.

### 2b. Rearrange Texture

The Model panel's **Rearrange Texture** button writes `name_rearranged.glb` and `.png` beside the model, never
overwriting, and opens both. Each connected piece of a section is flattened with least squares conformal maps, refined
as-rigid-as-possible to even out stretch, turned lengthwise with its inner end on the left, and packed in section
order. Triangles keep their order, and each new pixel reads the old texture through its own triangle's old UVs.

First it removes surfaces nothing can ever see, unless **Keep Hidden Surfaces** is ticked: a triangle stays if at least
2 of 32 rays over its hemisphere escape the model. Generated models are often hollow thin shells, and on the peel 48%
of the triangles were the inside of that shell, 0.002 from a visible face and pointing the opposite way.

A piece that folds over itself, often because of tunnels in a generated mesh, is halved across its length and each half
flattened again, up to four times.

On the 139,428-triangle peel this takes about 4 s and leaves 71,951 triangles in 50 islands, with median area stretch
1.03, 228 flipped triangles and 8,651 overlapping pixels out of 4 million. Most of the islands are specks along the
bottom. `cargo run -p qsketch-3d --example rearrange_report` prints the numbers.

Known limits: splitting a fold cuts that part into several islands. Sections are found from paint only during
Rearrange; loading a model still uses geometry alone.

### 3. Overlays

Canvas overlay modes, each toggled on its own: island outlines, tint by section, tint by facing, labels, and mapped
pixels only (everything outside the UVs shown black). Drawn from a texture rendered in UV space from the mesh.

### 4. Paint limits

**Select on Canvas** turns the selected section into the document's selection, as one undoable step, so every brush,
fill and filter respects it. The mask grows 3 pixels past the section into unclaimed texture, so paint reaches past the
rim without showing a seam, and never into another section. The section's tint is cleared at the same time, so it
does not cover the paint.

### 6. Simplifier

Generated meshes are far too dense for games: the peel is 139,428 triangles and wants about 4,000. Reduce with
quadric edge collapse, keep section boundaries where possible, create a clean new UV layout per section, and bake the
old texture onto it. Clean islands then make the older tools work better too.

## Polish pass (2026-11-16)

Found by using the panel by hand. Ticked as each lands on `main`.

- [x] Extract Texture, panning, larger panel, clearer errors (0.72.1)
- [x] The sectioner over-splits compact low-poly models (a 1,680-triangle creature came out as 16 parts): left as
      it is and handled by merging. A peel strip and a creature's limb measure the same (slices about 1.5 times
      wider than thick on the peel, 2 to 9 on the creature), so no thinness test tells them apart.
- [x] Rename and merge sections, kept in a sidecar beside the model
- [x] Remember the model between sessions: models join the recent files, and the empty panel offers the last one
- [x] Find sections off the UI thread, and check the 987,308-triangle peel in the app: it opens with a spinner and
      no freeze, and the canvas highlight is one cached image (a selected section cost 40% of a core per frame as
      triangles)
- [x] Pick a section by clicking the canvas: Ctrl+Alt+right-click

## Open questions

- Only GLB is planned to start. OBJ and glTF with external files can follow if they are needed.
- Should texture documents be remembered per model between sessions?

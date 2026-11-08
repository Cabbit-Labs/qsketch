# q3d: paint textures with the 3D model beside you

**Status: in progress on the `q3d` branch. Nothing here has shipped.** Each step below is marked as it lands.

A texture atlas is hard to read: the pixels for one part of a model are scattered across many islands. q3d puts the
model in a dockable qSketch panel, showing the document as its live texture, so you can see what you are painting and
say which part of the model you mean.

It came out of painting a banana peel for a game mod, whose atlas has 888 UV islands. It is built in, not a bridge to
a separate viewer, because selecting and highlighting need the canvas and the 3D view to share state on every frame.

## Principles

- **The document stays an ordinary image.** Guides, tints and labels are drawn over the canvas and are never saved
  into it, so they cannot end up flattened into a PNG.
- **The model file is not modified.** Sections live in a small sidecar file next to it.
- **The core is headless.** `crates/qsketch-3d` (mesh loading, UV analysis, sectioning) has no GUI or GPU code and
  is unit tested. Drawing and panels live in `qsketch-app`.

## Steps

| # | Step | Status |
|---|------|--------|
| 1 | Model panel: open a GLB, orbit it, wear a document as a live texture | Done; live update during a stroke unverified |
| 2 | Sections: find them, list, pick on the model, highlight in both views | Mostly done; picking on the canvas and saving edits still to come |
| 2b | Rearrange Texture: one continuous island per section, with the paint baked across | Done; a few pieces still fold |
| 3 | Overlays: island outlines, tints, labels, mapped pixels only | Planned |
| 4 | Paint limits: paint only inside a section, through the selection system | Planned |
| 5 | Paint directly on the 3D model | Later |
| 6 | Simplifier: reduce a heavy mesh and bake its texture across | Later |

### 1. Model panel

- Ways in: the **Model** panel (Window menu), `qsketch model.glb`, dropping a `.glb` on the window, or File > Open.
- The panel samples the GPU texture the canvas already keeps for each document, so strokes appear on the model as
  they are made, with no save. The document's canvas must be visible, since only visible documents upload tiles.
- The 3D view renders off screen with its own depth buffer and MSAA, because egui's render pass has neither, and is
  then copied into the panel.
- Controls: orbit, zoom, spin, texture document (the active one or a fixed one), and rotate 90 degrees, since
  generated models often come in with the wrong axis up.
- Not yet: the model is not remembered between sessions, and only GLB with an embedded buffer loads.

### 2. Sections

A section is a named set of triangles, drawn from one UV island or several. A triangle's UV gives its canvas pixels,
so one selection can be shown in both views. Picking on the model tests the click against the projected triangles. Picking on the
canvas looks up an ID map rendered in UV space.

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
5. **Name and keep (not built yet):** let the user rename, merge and split, and save the result in the sidecar so the
   automatic pass runs once and those edits are kept. For now sections are named "Part A top" and so on, and are
   recomputed on load.

The peel comes out as four strips with a top and underside each, plus the stem and hub: nine sections in about
0.13 s for 139,428 triangles and 1.1 s for 987,308. `cargo run -p qsketch-3d --example section_report -- model.glb -z`
prints the result for a model. Sections span islands freely, so a poor UV layout does not limit them. A triangle-level split is allowed where one
island crosses parts, which is common on these models.

### 2b. Rearrange Texture

The Model panel's **Rearrange Texture** button writes `name_rearranged.glb` and `.png` beside the model, never
overwriting, and opens both. Each connected piece of a section is flattened with least squares conformal maps, refined
as-rigid-as-possible to even out stretch, turned lengthwise with its inner end on the left, and packed in section
order. Triangles keep their order, and each new pixel reads the old texture through its own triangle's old UVs.

On the 139,428-triangle peel this takes about 6 s and gives 25 islands, median area stretch 1.06, 924 flipped triangles
and 82,322 overlapping pixels out of 4 million. `cargo run -p qsketch-3d --example rearrange_report` prints the numbers.

Known limits: a piece with tunnels through it, common on generated meshes, can fold over itself, and one yellow island
on the peel does at its root. The next fix is to split any piece that folds and flatten the parts separately. Sections
are found from paint only during Rearrange; loading a model still uses geometry alone.

### 3. Overlays

Canvas overlay modes, each toggled on its own: island outlines, tint by section, tint by facing, labels, and mapped
pixels only (everything outside the UVs shown black). Drawn from a texture rendered in UV space from the mesh.

### 4. Paint limits

Turn the selected section into a selection mask, so every brush, fill and filter respects it. Edge padding, painting
slightly past each island so seams don't show, is applied to the mask. Selections already support this kind of mask.

### 6. Simplifier

Generated meshes are far too dense for games: the peel is 139,428 triangles and wants about 4,000. Reduce with
quadric edge collapse, keep section boundaries where possible, create a clean new UV layout per section, and bake the
old texture onto it. Clean islands then make the older tools work better too.

## Open questions

- Saving a section sidecar: beside the model, or inside the `.qsk`? Leaning beside the model, with the `.qsk` keeping a
  link to it.
- Only GLB is planned to start. OBJ and glTF with external files can follow if they are needed.
- Should texture documents be remembered per model between sessions?

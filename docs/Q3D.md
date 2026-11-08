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
| 2 | Sections: list, pick on model or canvas, highlight in both views | Planned |
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
so one selection can be shown in both views. Picking on the model is a ray cast against the mesh. Picking on the
canvas looks up an ID map rendered in UV space.

### The sectioner (generated models)

Hand-made models have a few clean islands, and those can be sections as they stand. Generated models do not: the
peel has 888 islands, and many are scraps a few pixels wide. The sectioner turns that into a handful of useful parts:

1. **Find islands** in UV space, from shared edges, and connected components in 3D, welding positions that UV seams split.
2. **Absorb scraps:** merge islands below an area threshold into the neighbor they share the longest 3D edge with.
3. **Find parts:** split the welded surface where it pinches or creases sharply, using the classic approach of growing
   regions from flat seeds until curvature stops them. The peel's four strips and stem are the test case.
4. **Split by facing:** within a part, optionally separate surfaces by normal direction ("A top" and "A under").
5. **Name and keep:** suggest names from position and facing, and let the user rename, merge and split. The result is
   saved in the sidecar, so the automatic pass runs once and the user's edits are not thrown away.

Sections span islands freely, so a poor UV layout does not limit them. A triangle-level split is allowed where one
island crosses parts, which is common on these models.

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

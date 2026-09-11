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
| Export Image… | `Ctrl+Shift+E` |
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
| Hue/Saturation… | `Ctrl+U` |

## Layer

| Command | Default shortcut |
| --- | --- |
| New Layer | `Ctrl+Shift+N` |
| Duplicate Layer | `Ctrl+J` |
| Delete Layer | — |
| Merge Down | `Ctrl+E` |
| Merge Visible | `Ctrl+Shift+E` |
| Flatten Image | — |
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
| Flip Layer Horizontal | — |
| Flip Layer Vertical | — |
| Clear Layer | — |

## Select

| Command | Default shortcut |
| --- | --- |
| All | `Ctrl+A` |
| Deselect | `Ctrl+D` |
| Inverse | `Ctrl+Shift+I` |
| Select Layer Content | — |

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
| Fullscreen | `F11` |
| Hide/Show Panels | `Tab` |

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
| Move Tool | `V` |
| Rectangular Marquee | `M` |
| Elliptical Marquee | `Shift+M` |
| Lasso | `L` |
| Magic Wand | `W` |
| Crop | `C` |
| Eyedropper | `I` |
| Brush | `B` |
| Pencil | `N` |
| Eraser | `E` |
| Paint Bucket | `G` |
| Gradient | `Shift+G` |
| Line | `U` |
| Rectangle | `R` |
| Ellipse | `Alt+U` |
| Contour | `P` |
| Text | `T` |
| Zoom | `Z` |
| Hand | `H` |
| Rotate View | `Shift+R` |
| Quick Rotate (while held; move the pointer) | `Q` |
| Reset rotation | `Q` `Q` (double-tap) |

## Brush

| Command | Default shortcut |
| --- | --- |
| Increase Brush Size | `]` |
| Decrease Brush Size | `[` |
| Increase Hardness | `Shift+]` |
| Decrease Hardness | `Shift+[` |
| Swap Foreground/Background | `X` |
| Default Colors | `D` |
| Mirror Horizontal | `Shift+H` |
| Mirror Vertical | `Shift+V` |
| Symmetry Off | — |
| Set Symmetry Center… | — |
| Reset Symmetry Center | — |

## Window

| Command | Default shortcut |
| --- | --- |
| Tools | — |
| Layers | `F7` |
| History | — |
| Color | `F6` |
| Swatches | — |
| Navigator | — |
| Brushes | `F5` |
| Info | `F8` |
| Reset Workspace | — |

## Help

| Command | Default shortcut |
| --- | --- |
| Keyboard Shortcuts… | `Ctrl+Alt+Shift+K` |
| About qsketch | — |

# qsketch application icon set

A modern, flat "q" mark: a rounded-square tile in a charcoal-to-indigo
gradient, with a bold lowercase **q** whose descender tapers into a
brush/pen stroke, rendered in a teal → lime accent gradient.

## Files

| File | Purpose |
|---|---|
| `icon.svg` | Master vector source, 512x512 viewBox. Hand-authored SVG (paths, gradients, clip-path) — edit this to change the design. |
| `icon-16.png` … `icon-1024.png` | RGBA PNGs at 16, 32, 48, 64, 128, 256, 512, 1024 px. Transparent outside the rounded tile. |
| `icon.ico` | Windows multi-size icon, embeds 16/32/48/64/128/256 px PNG frames. |
| `gen_icons.py` | Regenerates every PNG and `icon.ico` from `icon.svg`. |

## Design

- Tile: rounded square, corner radius ≈22% of the tile size (113px at
  512px), background linear gradient `#1e1f2a → #2b2d42`, plus a very
  subtle white top-highlight gradient (glass effect, ~10% opacity max).
- Glyph: the bowl of the "q" is an outer/inner circle pair (an
  even-odd "ring" path); the descender is a single tapered polygon
  (wide where it meets the bowl, narrowing to a fine point) generated
  along a cubic-Bezier centerline so it reads as one continuous
  brush/pen stroke. Both the bowl and the stem share one accent
  gradient (`#17e3b4 → #8be34a → #d9f24e`, teal to lime) so the glyph
  reads as a single mark.
- Verified legible at 16x16: the ring's counter and the stroke both
  stay visible at the smallest target size; no detail thinner than
  survives 4x-supersampled downscaling.

## Regenerating the PNGs / ICO

```
python3 gen_icons.py
```

Requires:
- PyGObject + librsvg introspection bindings, for faithful (gradient-
  and clip-path-correct) SVG rasterization:
  `apt install gir1.2-rsvg-2.0 python3-gi python3-cairo`
- Pillow (`pip install pillow`) for downsampling and `.ico` packaging.

Why not plain ImageMagick `convert icon.svg icon-256.png`? This
machine's ImageMagick has no `rsvg-convert`/Inkscape delegate
installed, so it falls back to its own built-in MSVG renderer, which
does not resolve `<linearGradient>`/`<radialGradient>` fills (they
render as flat black). `gen_icons.py` instead drives librsvg directly
through GObject-Introspection (`gi.repository.Rsvg`) + `cairo`, which
renders gradients and the clip-path correctly, then supersamples 4x
and downsamples with Pillow's LANCZOS filter for clean edges at every
target size.

## Provenance

The exact bowl/stem geometry in `icon.svg` was produced by a small
one-off Python helper (not shipped) that samples a tapered-stroke
polygon along a cubic Bezier and emits it as static SVG path data;
the resulting `icon.svg` is a plain, dependency-free static SVG with
no build step of its own — only `gen_icons.py` (rasterization) has
tooling dependencies.

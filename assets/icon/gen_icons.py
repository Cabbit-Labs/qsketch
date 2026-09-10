#!/usr/bin/env python3
"""Regenerate all qsketch icon PNGs + the multi-size .ico from icon.svg.

Usage:
    python3 gen_icons.py

Requires:
    - PyGObject + librsvg (GObject-introspection "Rsvg" 2.0) for faithful
      SVG rasterization (gradients, clip-paths). On Debian/Ubuntu:
          apt install gir1.2-rsvg-2.0 python3-gi python3-cairo
    - Pillow (PIL) for downscaling / compositing / .ico packaging.
      pip install pillow

Each raster size is rendered at 4x supersample directly from the vector
source and then downsampled with LANCZOS, which gives much cleaner edges
on the rounded tile and the glyph than rendering straight at small sizes.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SVG_PATH = os.path.join(HERE, "icon.svg")

PNG_SIZES = [16, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 32, 48, 64, 128, 256]
SUPERSAMPLE = 4


def render_svg_to_surface(svg_path, size):
    import gi
    gi.require_version("Rsvg", "2.0")
    from gi.repository import Rsvg
    import cairo

    handle = Rsvg.Handle.new_from_file(svg_path)
    dim = handle.get_dimensions()
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    ctx = cairo.Context(surface)
    ctx.scale(size / dim.width, size / dim.height)
    handle.render_cairo(ctx)
    return surface


def render_png(svg_path, out_path, size, supersample=SUPERSAMPLE):
    from PIL import Image
    import io

    big = size * supersample
    surface = render_svg_to_surface(svg_path, big)
    buf = io.BytesIO()
    surface.write_to_png(buf)
    buf.seek(0)

    # cairo's write_to_png() already un-premultiplies alpha and encodes a
    # standard RGBA PNG, so no channel-order fixup is needed here.
    im = Image.open(buf).convert("RGBA")
    if supersample > 1:
        im = im.resize((size, size), Image.LANCZOS)
    im.save(out_path)
    return im


def main():
    if not os.path.isfile(SVG_PATH):
        print(f"error: {SVG_PATH} not found", file=sys.stderr)
        sys.exit(1)

    from PIL import Image

    generated = {}
    for size in PNG_SIZES:
        out_path = os.path.join(HERE, f"icon-{size}.png")
        print(f"rendering {out_path} ({size}x{size}, {SUPERSAMPLE}x supersample)")
        generated[size] = render_png(SVG_PATH, out_path, size)

    ico_path = os.path.join(HERE, "icon.ico")
    print(f"writing {ico_path} sizes={ICO_SIZES}")
    base = generated[max(ICO_SIZES)]
    ico_images = [generated[s] for s in ICO_SIZES if s in generated]
    base.save(
        ico_path,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=ico_images[1:],
    )

    print("done.")


if __name__ == "__main__":
    main()

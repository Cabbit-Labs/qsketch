use qsketch_core::raster::ResizeFilter;
use qsketch_core::warp::{warp_raster, Mesh};
use qsketch_core::{IRect, Pt, Raster, Rgba8};

fn main() {
    // Source: thick outlined letter-like strokes on transparent.
    let mut r = Raster::new(160, 80);
    for (x, w) in [(10, 8), (40, 8), (70, 8), (100, 8), (130, 8)] {
        r.fill_rect(IRect::new(x, 10, w, 60), Rgba8::rgb(0, 0, 0));
        r.fill_rect(IRect::new(x + 2, 12, w - 4, 56), Rgba8::rgb(230, 230, 230));
    }
    r.fill_rect(IRect::new(10, 36, 128, 8), Rgba8::rgb(0, 0, 0));
    let q = [Pt::new(20.0, 20.0), Pt::new(340.0, 20.0), Pt::new(340.0, 180.0), Pt::new(20.0, 180.0)];
    let mut mesh = Mesh::from_quad(q, 3, 3);
    mesh.pts[5].y += 25.0;
    mesh.pts[6].y -= 20.0;
    mesh.pts[10].x += 15.0;
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/warp".into());
    for (f, name) in [(ResizeFilter::Bilinear, "smooth"), (ResizeFilter::Nearest, "pixel")] {
        let (img, rect) = warp_raster(&r, &mesh, f, IRect::new(0, 0, 400, 220));
        let mut canvas = Raster::new(400, 220);
        canvas.fill_rect(IRect::new(0, 0, 400, 220), Rgba8::rgb(255, 255, 255));
        for y in 0..rect.h {
            for x in 0..rect.w {
                let p = img.get_pixel(x, y);
                if p.a > 0 {
                    let d = canvas.get_pixel(rect.x + x, rect.y + y);
                    let a = p.a as f32 / 255.0;
                    let mix = |s: u8, b: u8| (s as f32 * a + b as f32 * (1.0 - a)) as u8;
                    canvas.set_pixel(rect.x + x, rect.y + y, Rgba8::rgb(mix(p.r, d.r), mix(p.g, d.g), mix(p.b, d.b)));
                }
            }
        }
        let path = format!("{out}_{name}.png");
        image::save_buffer(&path, &canvas.to_rgba(), 400, 220, image::ColorType::Rgba8).unwrap();
        println!("{path}");
    }
}

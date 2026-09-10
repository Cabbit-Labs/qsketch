//! Quick throughput check: `cargo run --release -p qsketch-core --example bench`

use std::time::Instant;

use qsketch_core::{BrushSettings, Document, PaintMode, Pt, Rgba8, StrokeEngine, StrokeSample};

fn main() {
    for (w, h, size) in [(1920u32, 1080u32, 25.0f32), (4096, 4096, 25.0), (4096, 4096, 300.0)] {
        let mut doc = Document::new(w, h, Some(Rgba8::WHITE), "bench");
        let t0 = Instant::now();
        doc.mark_all_dirty();
        doc.update_composite();
        let full = t0.elapsed();

        let settings = BrushSettings { size, hardness: 0.8, flow: 0.7, pressure_size: false, ..Default::default() };
        let layer = doc.state().active_layer().clone();
        let mut engine = StrokeEngine::new(settings, PaintMode::Paint, Rgba8::BLACK, &layer, None);
        let t1 = Instant::now();
        let n = 400;
        for i in 0..n {
            let t = i as f32 / n as f32;
            let p = Pt::new(100.0 + t * (w as f32 - 200.0), h as f32 * 0.5 + (t * 12.0).sin() * 200.0);
            let raster = &mut doc.state_mut().layers[0].raster;
            let dirty = engine.extend(raster, StrokeSample { pos: p, pressure: 1.0 });
            doc.mark_dirty_rect(dirty);
        }
        let paint = t1.elapsed();
        let t2 = Instant::now();
        doc.update_composite();
        let recomposite = t2.elapsed();
        let t3 = Instant::now();
        doc.commit("stroke");
        let commit = t3.elapsed();
        println!(
            "{w}x{h} brush {size:>4.0}px: full composite {:>7.1?} | {} dabs in {:>7.1?} ({:.0} dabs/s) | dirty recomposite {:>7.1?} | commit {:?}",
            full,
            engine.dab_count(),
            paint,
            engine.dab_count() as f64 / paint.as_secs_f64(),
            recomposite,
            commit
        );
    }
}

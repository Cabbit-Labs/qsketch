//! Prints what the sectioner finds in a `.glb`, for tuning it on real models.
//!
//!     cargo run -p qsketch-3d --example section_report -- model.glb [up: +x|-x|+y|-y|+z|-z]

use std::time::Instant;

use qsketch_3d::{sections::find_sections, Mesh};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or_else(|| anyhow::anyhow!("usage: section_report model.glb [up]"))?;
    let up = match args.next().as_deref() {
        Some("+x") => [1.0, 0.0, 0.0],
        Some("-x") => [-1.0, 0.0, 0.0],
        Some("-y") => [0.0, -1.0, 0.0],
        Some("+z") => [0.0, 0.0, 1.0],
        Some("-z") => [0.0, 0.0, -1.0],
        _ => [0.0, 1.0, 0.0],
    };
    let mesh = Mesh::from_glb(&std::fs::read(path)?)?;
    let start = Instant::now();
    let found = find_sections(&mesh, up, None);
    println!("{} triangles, {} sections, {:?}", mesh.triangle_count(), found.list.len(), start.elapsed());
    for s in &found.list {
        println!(
            "{:>16}  {:>8} triangles  {:>5.1}%",
            s.name,
            s.triangles,
            100.0 * s.triangles as f32 / mesh.triangle_count() as f32
        );
    }
    Ok(())
}

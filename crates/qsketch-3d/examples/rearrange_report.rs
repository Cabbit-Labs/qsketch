//! Runs the section-based texture rearrangement on a model, for checking it without the app.
//!
//!     cargo run -p qsketch-3d --example rearrange_report -- model.glb texture.png out.png [up]

use std::time::Instant;

use qsketch_3d::hidden::{find_hidden, without_triangles};
use qsketch_3d::unwrap::{rearrange, Texture};
use qsketch_3d::{find_sections, Mesh};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, texture, out, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: rearrange_report model.glb texture.png out.png [up]");
    };
    let up = match rest.first().map(String::as_str) {
        Some("-z") => [0.0, 0.0, -1.0],
        Some("+z") => [0.0, 0.0, 1.0],
        _ => [0.0, 1.0, 0.0],
    };
    let mesh = Mesh::from_glb(&std::fs::read(model)?)?;
    let hidden = find_hidden(&mesh);
    println!("{} of {} triangles hidden", hidden.iter().filter(|h| **h).count(), hidden.len());
    let mesh = without_triangles(&mesh, &hidden);
    let image = image::open(texture)?.to_rgba8();
    let source = Texture { width: image.width(), height: image.height(), rgba: image.as_raw() };
    let sections = find_sections(&mesh, up, Some(&source));
    for s in &sections.list {
        println!("{:>16} {:>7}", s.name, s.triangles);
    }
    let start = Instant::now();
    let result = rearrange(&mesh, &sections, &source, 2048);
    println!("{} sections, {:?}", sections.list.len(), start.elapsed());
    println!("{:#?}", result.report);
    image::save_buffer(out, &result.rgba, result.size, result.size, image::ColorType::Rgba8)?;
    Ok(())
}

//! Parse Aseprite files and print a summary (dev aid).
fn main() {
    for p in std::env::args().skip(1) {
        let bytes = std::fs::read(&p).expect("read");
        match qsketch_core::io::ase::parse(&bytes) {
            Ok(s) => {
                println!(
                    "{p}: {}x{} depth={} frames={} layers={} tags={} palette={}",
                    s.width,
                    s.height,
                    s.depth,
                    s.frames.len(),
                    s.layers.len(),
                    s.tags.len(),
                    s.palette.len()
                );
                for l in &s.layers {
                    println!(
                        "  layer {:?} kind={} level={} blend={} op={} flags={:#x}",
                        l.name, l.kind, l.child_level, l.blend, l.opacity, l.flags
                    );
                }
                for (i, f) in s.frames.iter().enumerate().take(3) {
                    println!("  frame {i}: {}ms cels={}", f.duration_ms, f.cels.len());
                }
                match s.to_doc(0) {
                    Ok((d, w)) => {
                        let nonempty = d.layers.iter().filter(|l| !l.raster.is_empty()).count();
                        println!("  doc: {} layers ({} with pixels), warnings={w:?}", d.layers.len(), nonempty);
                        let out = std::env::temp_dir().join("ase_dump_rt.ase");
                        qsketch_core::io::ase::save(&out, &d).expect("save");
                        let back = qsketch_core::io::ase::load(&out).expect("reload");
                        let same =
                            back.layers.len() == d.layers.len()
                                && back.layers.iter().zip(&d.layers).all(|(a, b)| {
                                    a.props.name == b.props.name && a.raster.to_rgba() == b.raster.to_rgba()
                                });
                        println!("  roundtrip ok={same}");
                    }
                    Err(e) => println!("  to_doc failed: {e:#}"),
                }
            }
            Err(e) => println!("{p}: parse failed: {e:#}"),
        }
    }
}

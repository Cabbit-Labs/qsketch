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
                match s.to_doc() {
                    Ok((d, w)) => {
                        let nonempty = d.layers.iter().filter(|l| !l.raster.is_empty()).count();
                        println!("  doc: {} layers ({} with pixels), warnings={w:?}", d.layers.len(), nonempty);
                        let out = std::env::temp_dir().join("ase_dump_rt.ase");
                        qsketch_core::io::ase::save(&out, &d).expect("save");
                        let back = qsketch_core::io::ase::load(&out).expect("reload");
                        let mut same = back.layers.len() == d.layers.len() && back.frame_count() == d.frame_count();
                        for f in 0..d.frame_count() {
                            for (li, (a, b)) in back.layers.iter().zip(&d.layers).enumerate() {
                                let x = back.cel_image(li, f).map(|r| r.to_rgba());
                                let y = d.cel_image(li, f).map(|r| r.to_rgba());
                                let links = (a.cel_owner(f), b.cel_owner(f));
                                if a.props.name != b.props.name || x != y || links.0 != links.1 {
                                    println!(
                                        "  frame {f} layer {li} ({:?}) differs after the round trip",
                                        a.props.name
                                    );
                                    same = false;
                                }
                            }
                        }
                        for (li, l) in d.layers.iter().enumerate() {
                            let owners: Vec<usize> = (0..d.frame_count()).map(|f| l.cel_owner(f)).collect();
                            let filled: Vec<bool> = (0..d.frame_count()).map(|f| d.cel_has_pixels(li, f)).collect();
                            println!("  layer {li} {:?}: owners={owners:?} filled={filled:?}", l.props.name);
                        }
                        for t in &d.tags {
                            println!(
                                "  tag {:?} {}..={} {:?} repeat={} color={:?}",
                                t.name, t.from, t.to, t.direction, t.repeat, t.color
                            );
                        }
                        println!("  roundtrip ok={same}");
                    }
                    Err(e) => println!("  to_doc failed: {e:#}"),
                }
            }
            Err(e) => println!("{p}: parse failed: {e:#}"),
        }
    }
}

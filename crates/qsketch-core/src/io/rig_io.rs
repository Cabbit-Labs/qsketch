//! Game-ready export of a skeletal rig: one JSON file with the bones, the
//! parts (layer pictures) and the animations, plus a PNG per part.
//!
//! The format is documented in `docs/RIG_FORMAT.md`. In short: a part is drawn
//! with `world(bone) × transform`, where `world(bone)` is the bone's
//! animated world matrix and `transform` maps the part's pixels (top-left
//! is 0,0) into the bone's local space as set up. A runtime needs no
//! knowledge of how the art was drawn.

use std::path::Path;

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::document::DocState;
use crate::rig::{BoneId, Curve, Key, Xf};

/// What an export wrote.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RigExportSummary {
    pub bones: usize,
    pub parts: usize,
    pub animations: usize,
    /// Visible layers left out (groups, adjustments, empty layers).
    pub skipped: Vec<String>,
    /// Extra pictures written for parts whose art differs between the
    /// document's frames (see "Frame art" in the format doc).
    pub frame_images: usize,
}

/// Make `name` unique among `used` by appending " 2", " 3", …
fn unique(used: &mut Vec<String>, name: &str, fallback: &str) -> String {
    let base = if name.trim().is_empty() { fallback.to_string() } else { name.trim().to_string() };
    let mut out = base.clone();
    let mut n = 2;
    while used.contains(&out) {
        out = format!("{base} {n}");
        n += 1;
    }
    used.push(out.clone());
    out
}

/// A file-name-safe stem.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || "-_".contains(c) { c } else { '_' }).collect();
    if s.trim_matches('_').is_empty() {
        "part".into()
    } else {
        s
    }
}

fn curve_json(c: Curve) -> Value {
    match c {
        Curve::Linear => json!("linear"),
        Curve::Stepped => json!("stepped"),
        Curve::Bezier(b) => json!(b),
    }
}

fn keys_json<T>(keys: &[Key<T>], value: impl Fn(&T) -> Value) -> Value {
    Value::Array(
        keys.iter()
            .map(|k| {
                let mut o = value(&k.value);
                o["time"] = json!(k.time_ms as f64 / 1000.0);
                o["curve"] = curve_json(k.curve);
                o
            })
            .collect(),
    )
}

/// Write `<dir>/rig.json` and `<dir>/parts/*.png` for `doc`'s rig, with the
/// layers as they are drawn now (the setup pose).
pub fn export(doc: &DocState, dir: &Path) -> anyhow::Result<RigExportSummary> {
    let rig = &doc.rig;
    if rig.bones.is_empty() {
        bail!("this document has no bones");
    }
    let mut doc = doc.clone();
    doc.sync_cels();
    let nframes = doc.frame_count();
    let cur_frame = doc.frame;
    let mut frame_images = 0usize;
    std::fs::create_dir_all(dir.join("parts")).with_context(|| format!("creating {}", dir.display()))?;

    // Bones, with unique names; parents come first in `rig.bones`.
    let mut bone_names: Vec<String> = Vec::new();
    let mut by_id: Vec<(BoneId, String)> = Vec::new();
    for b in &rig.bones {
        let n = unique(&mut bone_names, &b.name, "bone");
        by_id.push((b.id, n));
    }
    let name_of = |id: BoneId| by_id.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone());
    let bones: Vec<Value> = rig
        .bones
        .iter()
        .map(|b| {
            json!({
                "name": name_of(b.id),
                "parent": b.parent.and_then(name_of),
                "x": b.x, "y": b.y,
                "rotation": b.rotation,
                "scaleX": b.scale[0], "scaleY": b.scale[1],
                "length": b.length,
            })
        })
        .collect();

    // Parts: every visible layer with pixels, bottom to top.
    let setup = rig.pose(None);
    let mut part_names: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut parts = Vec::new();
    let mut part_by_layer: Vec<(crate::layer::LayerId, String)> = Vec::new();
    let mut skipped = Vec::new();
    for i in 0..doc.layers.len() {
        if !doc.effectively_visible(i) {
            continue;
        }
        let l = &doc.layers[i];
        if !l.owns_pixels() {
            skipped.push(l.name().to_string());
            continue;
        }
        let mut l = l.clone();
        // Frame art: when the layer's picture differs between the document's
        // frames, each distinct picture (by the frame that owns it) is
        // written and the part lists which one every frame shows.
        let mut owners: Vec<usize> = (0..nframes).map(|f| l.cel_owner(f)).collect();
        let mut pictures: Vec<(usize, crate::raster::Raster)> = Vec::new();
        if nframes > 1 && l.animated() && owners.iter().any(|o| *o != owners[0]) {
            // Frames that own separate copies of the same pixels count as one
            // picture: compare what they actually show.
            let mut shown: Vec<(usize, Vec<u8>)> = Vec::new();
            for o in owners.iter_mut() {
                if pictures.iter().any(|(p, _)| p == o) {
                    continue;
                }
                let pic = l.cel_image(*o, cur_frame).cloned().unwrap_or_else(|| l.raster.clone());
                let mut tmp = l.clone();
                tmp.raster = pic;
                tmp.apply_mask();
                let bytes = tmp.raster.to_rgba();
                match shown.iter().find(|(_, b)| *b == bytes) {
                    Some((same, _)) => *o = *same,
                    None => {
                        shown.push((*o, bytes));
                        pictures.push((*o, tmp.raster));
                    }
                }
            }
        }
        let swaps = pictures.len() > 1;
        if !swaps {
            pictures.clear();
        }
        l.apply_mask();
        // One rectangle for all of a part's pictures, so they share a transform.
        let bounds = pictures.iter().filter_map(|(_, p)| p.full_bounds()).chain(l.raster.full_bounds());
        let Some(content) = bounds.reduce(|a, b| a.union(&b)) else {
            skipped.push(l.name().to_string());
            continue;
        };
        // A meshed part's picture is exactly its mesh's rectangle.
        let mesh = rig.mesh_of(l.id());
        let b = mesh.map_or(content, |m| m.rect);
        let name = unique(&mut part_names, l.name(), "part");
        part_by_layer.push((l.id(), name.clone()));
        let mut file = format!("parts/{}.png", file_stem(&name));
        let mut n = 2;
        while files.contains(&file) {
            file = format!("parts/{}_{n}.png", file_stem(&name));
            n += 1;
        }
        files.push(file.clone());
        let mut images: Vec<String> = Vec::new();
        if swaps {
            let stem = file.trim_end_matches(".png").to_string();
            let mut by_owner: Vec<(usize, String)> = Vec::new();
            for (o, pic) in &pictures {
                let f = format!("{stem}_f{}.png", o + 1);
                crate::io::image_io::export_raster(&dir.join(&f), &pic.crop(b))?;
                by_owner.push((*o, f));
            }
            frame_images += by_owner.len();
            images = owners
                .iter()
                .map(|o| by_owner.iter().find(|(p, _)| p == o).map(|(_, f)| f.clone()).unwrap_or_default())
                .collect();
            // The part's own image is the current frame's.
            file = images[cur_frame.min(images.len() - 1)].clone();
        } else {
            crate::io::image_io::export_raster(&dir.join(&file), &l.raster.crop(b))?;
        }

        let bone = rig.attached_bone(l.id());
        let place = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: b.x as f32, f: b.y as f32 };
        let transform = match bone.and_then(|id| rig.bone_index(id)).and_then(|bi| setup[bi].inverse()) {
            Some(inv) => inv * place,
            None => place,
        };
        let mut part = json!({
            "name": name,
            "bone": bone.and_then(name_of),
            "image": file,
            "width": b.w, "height": b.h,
            "x": b.x, "y": b.y,
            "transform": [transform.a, transform.b, transform.c, transform.d, transform.e, transform.f],
            "opacity": l.props.opacity,
            "blend": format!("{:?}", l.props.blend).to_lowercase(),
        });
        if !images.is_empty() {
            part["images"] = json!(images);
        }
        let rest = rig.rest_alpha_of(l.id());
        if rest < 0.999 {
            part["alpha"] = json!(rest);
        }
        if let Some(m) = mesh {
            // Bones are listed once, vertices refer to them by position:
            // each vertex is [bone, weight, bone, weight, ...].
            let mut palette: Vec<BoneId> = Vec::new();
            let vertices: Vec<Value> = m
                .weights
                .iter()
                .map(|w| {
                    let mut flat = Vec::new();
                    for inf in w {
                        if name_of(inf.bone).is_none() {
                            continue;
                        }
                        let at = palette.iter().position(|b| *b == inf.bone).unwrap_or_else(|| {
                            palette.push(inf.bone);
                            palette.len() - 1
                        });
                        flat.push(json!(at));
                        flat.push(json!(inf.weight));
                    }
                    Value::Array(flat)
                })
                .collect();
            let bones: Vec<String> = palette.iter().filter_map(|b| name_of(*b)).collect();
            part["mesh"] = json!({ "cols": m.cols, "rows": m.rows, "bones": bones, "vertices": vertices });
        }
        parts.push(part);
    }

    // IK constraints, with unique names.
    let mut ik_names: Vec<String> = Vec::new();
    let mut ik_by_id: Vec<(u32, String)> = Vec::new();
    let mut ik: Vec<Value> = Vec::new();
    for c in &rig.ik {
        let name = unique(&mut ik_names, &c.name, "ik");
        ik_by_id.push((c.id, name.clone()));
        let (Some(p), Some(ch), Some(t)) = (name_of(c.bones[0]), name_of(c.bones[1]), name_of(c.target)) else {
            continue;
        };
        ik.push(json!({ "name": name, "bones": [p, ch], "target": t, "bend": c.bend, "mix": c.mix }));
    }

    // Animations.
    let mut anim_names: Vec<String> = Vec::new();
    let mut animations = serde_json::Map::new();
    for a in &rig.animations {
        let mut tracks = serde_json::Map::new();
        for t in &a.tracks {
            let Some(bn) = name_of(t.bone) else { continue };
            let mut o = serde_json::Map::new();
            if !t.translate.is_empty() {
                o.insert("translate".into(), keys_json(&t.translate, |v| json!({ "x": v[0], "y": v[1] })));
            }
            if !t.rotate.is_empty() {
                o.insert("rotate".into(), keys_json(&t.rotate, |v| json!({ "angle": v })));
            }
            if !t.scale.is_empty() {
                o.insert("scale".into(), keys_json(&t.scale, |v| json!({ "x": v[0], "y": v[1] })));
            }
            if !o.is_empty() {
                tracks.insert(bn, Value::Object(o));
            }
        }
        let mut ik_tracks = serde_json::Map::new();
        for t in &a.ik {
            let Some((_, n)) = ik_by_id.iter().find(|(id, _)| *id == t.constraint) else { continue };
            if !t.keys.is_empty() {
                ik_tracks.insert(n.clone(), keys_json(&t.keys, |v| json!({ "mix": v.mix, "bend": v.bend })));
            }
        }
        let mut part_tracks = serde_json::Map::new();
        for t in &a.alpha {
            let Some((_, n)) = part_by_layer.iter().find(|(id, _)| *id == t.layer) else { continue };
            if !t.keys.is_empty() {
                part_tracks.insert(n.clone(), keys_json(&t.keys, |v| json!({ "alpha": v })));
            }
        }
        let name = unique(&mut anim_names, &a.name, "animation");
        animations.insert(
            name,
            json!({
                "duration": a.duration_ms as f64 / 1000.0,
                "loop": a.looped,
                "bones": Value::Object(tracks),
                "ik": Value::Object(ik_tracks),
                "parts": Value::Object(part_tracks),
            }),
        );
    }

    let mut out = json!({
        "format": "qsketch-rig",
        "version": 1,
        "canvas": { "width": doc.width, "height": doc.height },
        "bones": bones,
        "ik": ik,
        "parts": parts,
        "animations": Value::Object(animations),
    });
    if frame_images > 0 {
        let secs: Vec<f64> = (0..nframes).map(|f| doc.frame_duration(f) as f64 / 1000.0).collect();
        out["frames"] = json!(secs);
    }
    std::fs::write(dir.join("rig.json"), serde_json::to_vec_pretty(&out)?)?;
    Ok(RigExportSummary {
        bones: rig.bones.len(),
        parts: parts.len(),
        animations: rig.animations.len(),
        skipped,
        frame_images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::geom::{IRect, Pt};
    use crate::rig::{Curve, IkValue, KeyValue};

    #[test]
    fn exports_bones_parts_and_animations() {
        let mut doc = DocState::new(64, 32, None);
        doc.layers[0].raster.fill_rect(IRect::new(4, 4, 8, 6), Rgba8::new(255, 0, 0, 255));
        let id = doc.layers[0].id();
        let bone = doc.rig.add_bone("body", None, 8.0, 8.0, 10.0);
        doc.rig.attach(id, bone);
        let a = doc.rig.add_animation("walk", 1500);
        doc.rig.set_key(a, bone, 0, KeyValue::Rotate(0.0), Curve::Linear);
        doc.rig.set_key(a, bone, 500, KeyValue::Rotate(30.0), Curve::Bezier([0.25, 0.0, 0.75, 1.0]));

        let dir = tempfile::tempdir().unwrap();
        let sum = export(&doc, dir.path()).unwrap();
        assert_eq!((sum.bones, sum.parts, sum.animations), (1, 1, 1));

        let json: Value = serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap();
        assert_eq!(json["format"], "qsketch-rig");
        assert_eq!(json["bones"][0]["name"], "body");
        assert!(json["bones"][0]["parent"].is_null());
        let part = &json["parts"][0];
        assert_eq!(part["bone"], "body");
        assert_eq!((part["width"].as_i64(), part["height"].as_i64()), (Some(8), Some(6)));
        // The bone sits at (8,8) in the setup pose and the picture's corner
        // at (4,4), so in bone space the corner is (-4,-4).
        let t = part["transform"].as_array().unwrap();
        assert!((t[4].as_f64().unwrap() + 4.0).abs() < 1e-4 && (t[5].as_f64().unwrap() + 4.0).abs() < 1e-4);
        assert!(dir.path().join("parts/Layer_1.png").exists());
        let rot = &json["animations"]["walk"]["bones"]["body"]["rotate"];
        assert_eq!(rot[1]["time"], 0.5);
        assert_eq!(rot[1]["angle"], 30.0);
        assert_eq!(rot[0]["curve"], "linear");
        assert_eq!(rot[1]["curve"], json!([0.25, 0.0, 0.75, 1.0]));
        assert_eq!(json["animations"]["walk"]["duration"], 1.5);
    }

    /// A mini runtime written from docs/RIG_FORMAT.md, reading only the JSON
    /// (linear keys only): the world matrix of every bone of `anim` at `t`.
    fn runtime_pose(j: &Value, anim: &str, t: f32) -> Vec<(String, Xf)> {
        let f = |v: &Value| v.as_f64().unwrap() as f32;
        let sample = |keys: &Value, field: &str, t: f32, default: f32| -> f32 {
            let Some(k) = keys.as_array().filter(|k| !k.is_empty()) else { return default };
            let at = |i: usize| (f(&k[i]["time"]), f(&k[i][field]));
            if t <= at(0).0 {
                return at(0).1;
            }
            for i in 1..k.len() {
                if t <= at(i).0 {
                    let (t0, v0) = at(i - 1);
                    let (t1, v1) = at(i);
                    return v0 + (v1 - v0) * (t - t0) / (t1 - t0);
                }
            }
            at(k.len() - 1).1
        };
        struct L {
            name: String,
            parent: Option<usize>,
            x: f32,
            y: f32,
            rot: f32,
            sx: f32,
            sy: f32,
            length: f32,
        }
        // Loop: wrap the playhead by the duration; otherwise hold the end.
        let an = &j["animations"][anim];
        let dur = f(&an["duration"]);
        let t = if an["loop"] == true { t.rem_euclid(dur) } else { t.min(dur) };
        let bones = j["bones"].as_array().unwrap();
        let index = |n: &str| bones.iter().position(|b| b["name"] == n).unwrap();
        let mut locals: Vec<L> = bones
            .iter()
            .map(|b| {
                let name = b["name"].as_str().unwrap().to_string();
                let tr = &j["animations"][anim]["bones"][&name];
                L {
                    parent: b["parent"].as_str().map(index),
                    x: f(&b["x"]) + sample(&tr["translate"], "x", t, 0.0),
                    y: f(&b["y"]) + sample(&tr["translate"], "y", t, 0.0),
                    rot: f(&b["rotation"]) + sample(&tr["rotate"], "angle", t, 0.0),
                    sx: f(&b["scaleX"]) * sample(&tr["scale"], "x", t, 1.0),
                    sy: f(&b["scaleY"]) * sample(&tr["scale"], "y", t, 1.0),
                    length: f(&b["length"]),
                    name,
                }
            })
            .collect();
        let worlds = |l: &[L]| -> Vec<Xf> {
            let mut w: Vec<Xf> = Vec::new();
            for b in l {
                let local = Xf::trs(b.x, b.y, b.rot, b.sx, b.sy);
                w.push(b.parent.map_or(local, |p| w[p] * local));
            }
            w
        };
        let ang = |m: Xf| m.b.atan2(m.a);
        let wrap = |d: f32| (d + 540.0).rem_euclid(360.0) - 180.0;
        for c in j["ik"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let keys = &j["animations"][anim]["ik"][name];
            let mix = sample(keys, "mix", t, f(&c["mix"])).clamp(0.0, 1.0);
            // The bend of the last key at or before t (stepped).
            let bend = keys
                .as_array()
                .and_then(|k| k.iter().rev().find(|k| f(&k["time"]) <= t).or(k.first()))
                .map_or(c["bend"].as_f64().unwrap(), |k| k["bend"].as_f64().unwrap());
            let (pi, ci) = (index(c["bones"][0].as_str().unwrap()), index(c["bones"][1].as_str().unwrap()));
            let ti = index(c["target"].as_str().unwrap());
            let w = worlds(&locals);
            let (h, hc, tp) =
                (w[pi].apply(Pt::new(0.0, 0.0)), w[ci].apply(Pt::new(0.0, 0.0)), w[ti].apply(Pt::new(0.0, 0.0)));
            let a = h.dist(hc);
            let b = locals[ci].length * (w[ci].a * w[ci].a + w[ci].b * w[ci].b).sqrt();
            let d0 = h.dist(tp);
            if a < 1e-4 || b < 1e-4 || d0 < 1e-4 || mix <= 0.0 {
                continue;
            }
            let d = d0.clamp((a - b).abs().max(1e-4), a + b);
            let alpha = ((a * a + d * d - b * b) / (2.0 * a * d)).clamp(-1.0, 1.0).acos();
            let theta = (tp.y - h.y).atan2(tp.x - h.x) + if bend < 0.0 { -alpha } else { alpha };
            let offset = (hc.y - h.y).atan2(hc.x - h.x) - ang(w[pi]);
            let pp = locals[pi].parent.map_or(0.0, |p| ang(w[p]));
            let rot_p = (theta - offset - pp).to_degrees();
            let fk = locals[pi].rot;
            locals[pi].rot = fk + mix * wrap(rot_p - fk);
            let w = worlds(&locals);
            let hc = w[ci].apply(Pt::new(0.0, 0.0));
            let rot_c = ((tp.y - hc.y).atan2(tp.x - hc.x) - ang(w[pi])).to_degrees();
            let fk = locals[ci].rot;
            locals[ci].rot = fk + mix * wrap(rot_c - fk);
        }
        locals.iter().map(|l| l.name.clone()).zip(worlds(&locals)).collect()
    }

    fn exported(doc: &DocState) -> Value {
        let dir = tempfile::tempdir().unwrap();
        export(doc, dir.path()).unwrap();
        serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap()
    }

    /// The recipe in docs/RIG_FORMAT.md, implemented from the JSON alone,
    /// must put a part's corner where the editor's own posing puts it.
    #[test]
    fn the_documented_recipe_matches_the_editor() {
        let mut doc = DocState::new(64, 64, None);
        doc.layers[0].raster.fill_rect(IRect::new(30, 20, 10, 10), Rgba8::new(0, 255, 0, 255));
        let id = doc.layers[0].id();
        let root = doc.rig.add_bone("root", None, 20.0, 40.0, 10.0);
        let arm = doc.rig.add_bone("arm", Some(root), 12.0, -3.0, 10.0);
        doc.rig.bone_mut(arm).unwrap().rotation = 15.0;
        doc.rig.attach(id, arm);
        let a = doc.rig.add_animation("a", 1000);
        doc.rig.set_key(a, root, 0, KeyValue::Rotate(0.0), Curve::Linear);
        doc.rig.set_key(a, root, 1000, KeyValue::Rotate(60.0), Curve::Linear);
        doc.rig.set_key(a, arm, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        doc.rig.set_key(a, arm, 1000, KeyValue::Translate([8.0, 4.0]), Curve::Linear);
        let t_ms = 250;

        let j = exported(&doc);
        let world = runtime_pose(&j, "a", t_ms as f32 / 1000.0);
        let f = |v: &Value| v.as_f64().unwrap() as f32;
        let part = &j["parts"][0];
        let m = part["transform"].as_array().unwrap();
        let m = Xf { a: f(&m[0]), b: f(&m[1]), c: f(&m[2]), d: f(&m[3]), e: f(&m[4]), f: f(&m[5]) };
        let bone_world = world.iter().find(|(n, _)| n == part["bone"].as_str().unwrap()).unwrap().1;
        let got = (bone_world * m).apply(Pt::new(0.0, 0.0));

        // The editor's answer: the layer's corner moved by the bone's delta.
        let setup = doc.rig.pose(None);
        let pose = doc.rig.pose(Some((a, t_ms)));
        let delta = doc.rig.layer_delta(arm, &pose, &setup).unwrap();
        let want = delta.apply(Pt::new(30.0, 20.0));
        assert!((got.x - want.x).abs() < 1e-3 && (got.y - want.y).abs() < 1e-3, "{got:?} vs {want:?}");
    }

    /// The IK algorithm in docs/RIG_FORMAT.md, run from the JSON alone,
    /// must agree with the editor's solver (including a half mix, and a
    /// keyed mix that changes over time).
    #[test]
    fn the_documented_ik_matches_the_editor() {
        let mut doc = DocState::new(64, 64, None);
        let upper = doc.rig.add_bone("upper", None, 10.0, 30.0, 12.0);
        let fore = doc.rig.add_bone("fore", Some(upper), 12.0, 0.0, 14.0);
        doc.rig.bone_mut(upper).unwrap().rotation = -25.0;
        doc.rig.bone_mut(fore).unwrap().rotation = 50.0;
        let (ik, target) = doc.rig.add_ik(fore).unwrap();
        let a = doc.rig.add_animation("a", 1000);
        doc.rig.set_key(a, target, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        doc.rig.set_key(a, target, 1000, KeyValue::Translate([-9.0, 17.0]), Curve::Linear);
        doc.rig.set_key(a, upper, 0, KeyValue::Rotate(0.0), Curve::Linear);
        doc.rig.set_key(a, upper, 1000, KeyValue::Rotate(40.0), Curve::Linear);
        doc.rig.set_ik_key(a, ik, 0, IkValue { mix: 1.0, bend: doc.rig.ik[0].bend }, Curve::Linear);
        doc.rig.set_ik_key(a, ik, 1000, IkValue { mix: 0.5, bend: doc.rig.ik[0].bend }, Curve::Linear);

        let j = exported(&doc);
        assert_eq!(j["ik"][0]["target"], "fore target");
        for t_ms in [0u32, 300, 700, 1000] {
            let world = runtime_pose(&j, "a", t_ms as f32 / 1000.0);
            let pose = doc.rig.pose(Some((a, t_ms)));
            for (i, (name, w)) in world.iter().enumerate() {
                let want = pose[i];
                let close = |x: f32, y: f32| (x - y).abs() < 1e-2;
                assert!(
                    close(w.a, want.a) && close(w.b, want.b) && close(w.e, want.e) && close(w.f, want.f),
                    "{name} at {t_ms} ms: {w:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn frame_art_is_exported_as_image_swaps() {
        let mut doc = DocState::new(32, 16, None);
        doc.layers[0].raster.fill_rect(IRect::new(2, 2, 4, 4), Rgba8::new(255, 0, 0, 255));
        doc.rig.add_bone("b", None, 0.0, 0.0, 5.0);
        let dir = tempfile::tempdir().unwrap();
        // One frame: no swaps, nothing about frames in the file.
        assert_eq!(export(&doc, dir.path()).unwrap().frame_images, 0);
        let j: Value = serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap();
        assert!(j.get("frames").is_none() && j["parts"][0].get("images").is_none());

        // Three frames: the second draws further right, the third links to the first.
        doc.insert_frame(1, crate::NewFrame::Duplicate(0));
        doc.layers[0].raster.fill_rect(IRect::new(10, 2, 6, 4), Rgba8::new(0, 255, 0, 255));
        doc.insert_frame(2, crate::NewFrame::Duplicate(0));
        doc.link_cels(0, &[0, 2]);
        doc.set_frame_duration(1, 250);
        doc.set_frame(1);
        let sum = export(&doc, dir.path()).unwrap();
        assert_eq!(sum.frame_images, 2, "two distinct pictures");
        let j: Value = serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap();
        assert_eq!(j["frames"], json!([0.1, 0.25, 0.1]));
        let part = &j["parts"][0];
        let images: Vec<&str> = part["images"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0], images[2], "linked frames share a file");
        assert_ne!(images[0], images[1]);
        assert_eq!(part["image"], images[1], "the part's own image is the current frame's");
        // Every picture has the same rectangle: the union of both drawings.
        assert_eq!((part["x"].as_i64(), part["width"].as_i64()), (Some(2), Some(14)));
        for f in &images {
            let img = image_io_size(&dir.path().join(f));
            assert_eq!(img, (14, 4), "{f}");
        }
    }

    fn image_io_size(path: &std::path::Path) -> (u32, u32) {
        let r = crate::io::image_io::decode_bytes(&std::fs::read(path).unwrap()).unwrap();
        (r.width(), r.height())
    }

    #[test]
    fn a_document_without_bones_is_an_error() {
        let doc = DocState::new(8, 8, None);
        let dir = tempfile::tempdir().unwrap();
        assert!(export(&doc, dir.path()).is_err());
    }

    #[test]
    fn duplicate_names_are_made_unique() {
        let mut used = Vec::new();
        assert_eq!(unique(&mut used, "arm", "bone"), "arm");
        assert_eq!(unique(&mut used, "arm", "bone"), "arm 2");
        assert_eq!(unique(&mut used, " ", "bone"), "bone");
    }
}

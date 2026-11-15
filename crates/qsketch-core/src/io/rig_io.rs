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
        l.apply_mask();
        let Some(b) = l.raster.full_bounds() else {
            skipped.push(l.name().to_string());
            continue;
        };
        let name = unique(&mut part_names, l.name(), "part");
        let mut file = format!("parts/{}.png", file_stem(&name));
        let mut n = 2;
        while files.contains(&file) {
            file = format!("parts/{}_{n}.png", file_stem(&name));
            n += 1;
        }
        files.push(file.clone());
        crate::io::image_io::export_raster(&dir.join(&file), &l.raster.crop(b))?;

        let bone = rig.attached_bone(l.id());
        let place = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: b.x as f32, f: b.y as f32 };
        let transform = match bone.and_then(|id| rig.bone_index(id)).and_then(|bi| setup[bi].inverse()) {
            Some(inv) => inv * place,
            None => place,
        };
        parts.push(json!({
            "name": name,
            "bone": bone.and_then(name_of),
            "image": file,
            "width": b.w, "height": b.h,
            "x": b.x, "y": b.y,
            "transform": [transform.a, transform.b, transform.c, transform.d, transform.e, transform.f],
            "opacity": l.props.opacity,
            "blend": format!("{:?}", l.props.blend).to_lowercase(),
        }));
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
        let name = unique(&mut anim_names, &a.name, "animation");
        animations.insert(
            name,
            json!({ "duration": a.duration_ms as f64 / 1000.0, "loop": a.looped, "bones": Value::Object(tracks) }),
        );
    }

    let out = json!({
        "format": "qsketch-rig",
        "version": 1,
        "canvas": { "width": doc.width, "height": doc.height },
        "bones": bones,
        "parts": parts,
        "animations": Value::Object(animations),
    });
    std::fs::write(dir.join("rig.json"), serde_json::to_vec_pretty(&out)?)?;
    Ok(RigExportSummary { bones: rig.bones.len(), parts: parts.len(), animations: rig.animations.len(), skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::geom::IRect;
    use crate::rig::{Curve, KeyValue};

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

    /// The recipe in docs/RIG_FORMAT.md, implemented from the JSON alone,
    /// must put a part's corner where the editor's own posing puts it.
    #[test]
    fn the_documented_recipe_matches_the_editor() {
        use crate::geom::Pt;
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

        let dir = tempfile::tempdir().unwrap();
        export(&doc, dir.path()).unwrap();
        let j: Value = serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap();

        // A tiny runtime: linear keys only.
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
        let t = t_ms as f32 / 1000.0;
        let mut world: Vec<(String, Xf)> = Vec::new();
        for b in j["bones"].as_array().unwrap() {
            let name = b["name"].as_str().unwrap().to_string();
            let tr = &j["animations"]["a"]["bones"][&name];
            let (mut x, mut y) = (f(&b["x"]), f(&b["y"]));
            x += sample(&tr["translate"], "x", t, 0.0);
            y += sample(&tr["translate"], "y", t, 0.0);
            let rot = f(&b["rotation"]) + sample(&tr["rotate"], "angle", t, 0.0);
            let local = Xf::trs(x, y, rot, f(&b["scaleX"]), f(&b["scaleY"]));
            let w = match b["parent"].as_str() {
                Some(p) => world.iter().find(|(n, _)| n == p).unwrap().1 * local,
                None => local,
            };
            world.push((name, w));
        }
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

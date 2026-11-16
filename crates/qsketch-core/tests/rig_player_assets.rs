//! Builds the files the reference rig player (`examples/rig-player`) is
//! tested and demonstrated with, straight from the editor's own rig code:
//!
//! * `fixture/rig.json` + `fixture/expected.json`: a rig exercising every
//!   feature (Bézier and stepped easing, scale, IK with a keyed mix and bend,
//!   looping and non-looping animations) and the world matrices the editor
//!   computes for it at many times. `test.html` runs the JavaScript runtime
//!   against them.
//! * `sample/`: a little character (`rig.json` and its `parts/` PNGs) for
//!   `index.html`.
//!
//! This test fails when the committed files are out of date. Regenerate them
//! with `QSKETCH_UPDATE_PLAYER=1 cargo test -p qsketch-core --test rig_player_assets`.

use std::path::{Path, PathBuf};

use qsketch_core::io::rig_io;
use qsketch_core::rig::{BoneId, Curve, IkValue, KeyValue};
use qsketch_core::{DocState, IRect, Layer, Rgba8, Xf};
use serde_json::json;

fn player_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rig-player")
}

const EASE: Curve = Curve::Bezier([0.42, 0.0, 0.58, 1.0]);
const EASE_OUT: Curve = Curve::Bezier([0.0, 0.0, 0.58, 1.0]);

fn rot(doc: &mut DocState, a: usize, b: BoneId, keys: &[(u32, f32, Curve)]) {
    for &(t, v, c) in keys {
        doc.rig.set_key(a, b, t, KeyValue::Rotate(v), c);
    }
}

fn mov(doc: &mut DocState, a: usize, b: BoneId, keys: &[(u32, f32, f32, Curve)]) {
    for &(t, x, y, c) in keys {
        doc.rig.set_key(a, b, t, KeyValue::Translate([x, y]), c);
    }
}

fn add_part(doc: &mut DocState, name: &str, rect: IRect, color: Rgba8, bone: BoneId) {
    add_parts(doc, name, &[(rect, color)], bone, None);
}

/// A layer drawn from several rectangles, attached to `bone`; with
/// `Some(cell)` it also gets a mesh weighted to the nearby bones.
fn add_parts(doc: &mut DocState, name: &str, rects: &[(IRect, Rgba8)], bone: BoneId, mesh: Option<u32>) {
    let id = doc.new_id();
    let mut l = Layer::new(id, name, doc.width, doc.height);
    for &(rect, color) in rects {
        l.raster.fill_rect(rect, color);
    }
    let bounds = l.raster.full_bounds().unwrap();
    doc.layers.push(l);
    doc.rig.attach(id, bone);
    if let Some(cell) = mesh {
        assert!(doc.rig.add_mesh(id, bounds, cell));
        doc.rig.auto_weights(id, true);
    }
}

// --- the fixture -----------------------------------------------------------------

fn fixture_doc() -> (DocState, BoneId) {
    let mut doc = DocState::new(128, 128, None);
    doc.layers.clear();
    let root = doc.rig.add_bone("root", None, 60.0, 40.0, 20.0);
    let spine = doc.rig.add_bone("spine", Some(root), 20.0, 0.0, 20.0);
    doc.rig.bone_mut(spine).unwrap().rotation = -30.0;
    let tail = doc.rig.add_bone("tail", Some(spine), 20.0, 0.0, 15.0);
    let thigh = doc.rig.add_bone("thigh", Some(root), 0.0, 0.0, 30.0);
    doc.rig.bone_mut(thigh).unwrap().rotation = 80.0;
    let shin = doc.rig.add_bone("shin", Some(thigh), 30.0, 0.0, 28.0);
    doc.rig.bone_mut(shin).unwrap().rotation = -35.0;
    let upper = doc.rig.add_bone("upper", Some(spine), 10.0, 5.0, 22.0);
    doc.rig.bone_mut(upper).unwrap().rotation = 20.0;
    let lower = doc.rig.add_bone("lower", Some(upper), 22.0, 0.0, 18.0);
    doc.rig.bone_mut(lower).unwrap().rotation = -50.0;
    let (ik_leg, foot) = doc.rig.add_ik(shin).unwrap();
    let (ik_arm, hand) = doc.rig.add_ik(lower).unwrap();
    add_part(&mut doc, "body", IRect::new(40, 30, 16, 10), Rgba8::new(200, 60, 60, 255), spine);
    // A mesh weighted to all nine bones (only four per vertex survive).
    add_parts(&mut doc, "cloth", &[(IRect::new(30, 60, 30, 24), Rgba8::new(60, 160, 90, 255))], thigh, Some(8));
    let cloth = doc.layers.last().unwrap().id();
    doc.rig.auto_weights(cloth, false);

    let a = doc.rig.add_animation("loopy", 1200);
    mov(&mut doc, a, root, &[(0, 0.0, 0.0, EASE), (600, 6.0, -4.0, EASE_OUT), (1200, 0.0, 0.0, Curve::Linear)]);
    rot(&mut doc, a, spine, &[(0, 0.0, Curve::Stepped), (400, 15.0, Curve::Linear), (900, -10.0, EASE)]);
    for (t, sx, sy) in [(0, 1.0, 1.0), (500, 1.4, 0.7), (1200, 1.0, 1.0)] {
        doc.rig.set_key(a, tail, t, KeyValue::Scale([sx, sy]), EASE);
    }
    // The foot reaches, overreaches (straight leg) and comes back.
    mov(
        &mut doc,
        a,
        foot,
        &[
            (0, 0.0, 0.0, Curve::Linear),
            (400, -12.0, -20.0, EASE),
            (800, 30.0, 40.0, Curve::Linear),
            (1200, 0.0, 0.0, Curve::Linear),
        ],
    );
    mov(&mut doc, a, hand, &[(0, 0.0, 0.0, Curve::Linear), (1200, -10.0, 14.0, Curve::Linear)]);
    let bend = doc.rig.ik[0].bend;
    doc.rig.set_ik_key(a, ik_leg, 0, IkValue { mix: 1.0, bend }, Curve::Linear);
    doc.rig.set_ik_key(a, ik_leg, 600, IkValue { mix: 0.3, bend }, EASE);
    doc.rig.set_ik_key(a, ik_leg, 1000, IkValue { mix: 1.0, bend }, Curve::Linear);
    let bend = doc.rig.ik[1].bend;
    doc.rig.set_ik_key(a, ik_arm, 0, IkValue { mix: 1.0, bend }, Curve::Linear);
    doc.rig.set_ik_key(a, ik_arm, 600, IkValue { mix: 1.0, bend: -bend }, Curve::Linear);

    // Opacity: the cloth is half shown at rest, the body fades and snaps.
    let body = doc.layers.iter().find(|l| l.name() == "body").unwrap().id();
    doc.rig.set_rest_alpha(cloth, 0.5);
    doc.rig.set_alpha_key(a, body, 0, 1.0, Curve::Linear);
    doc.rig.set_alpha_key(a, body, 300, 0.2, EASE);
    doc.rig.set_alpha_key(a, body, 700, 0.9, Curve::Stepped);
    doc.rig.set_alpha_key(a, body, 1000, 0.0, Curve::Linear);
    doc.rig.set_alpha_key(a, cloth, 400, 1.0, Curve::Stepped);
    doc.rig.set_alpha_key(a, cloth, 800, 0.0, Curve::Linear);

    let b = doc.rig.add_animation("once", 800);
    doc.rig.animations[b].looped = false;
    rot(&mut doc, b, thigh, &[(0, 0.0, Curve::Linear), (400, -25.0, EASE), (800, 10.0, Curve::Linear)]);
    mov(&mut doc, b, foot, &[(0, 0.0, 0.0, Curve::Linear), (800, 5.0, -15.0, Curve::Linear)]);

    // Frame art: three frames of different lengths; the body is redrawn in each.
    let body_i = doc.layers.iter().position(|l| l.name() == "body").unwrap();
    for (f, c) in [(1, Rgba8::new(60, 200, 60, 255)), (2, Rgba8::new(60, 60, 200, 255))] {
        doc.insert_frame(f, qsketch_core::NewFrame::Duplicate(f - 1));
        doc.layers[body_i].raster.fill_rect(IRect::new(40, 30, 16, 10), c);
    }
    for (f, ms) in [(0, 100), (1, 200), (2, 150)] {
        doc.set_frame_duration(f, ms);
    }
    doc.set_frame(0);
    (doc, spine)
}

fn num(x: f32) -> serde_json::Value {
    json!(x)
}

fn matrix(m: Xf) -> serde_json::Value {
    json!([num(m.a), num(m.b), num(m.c), num(m.d), num(m.e), num(m.f)])
}

fn fixture_expected(doc: &DocState, spine: BoneId) -> String {
    let rig = &doc.rig;
    let layer = |n: &str| doc.layers.iter().find(|l| l.name() == n).unwrap().id();
    let (body, cloth) = (layer("body"), layer("cloth"));
    let setup = rig.pose(None);
    let place = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 40.0, f: 30.0 };
    let mut samples = Vec::new();
    let mut push = |anim: Option<usize>, t_ms: u32| {
        let pose = rig.pose(anim.map(|a| (a, t_ms)));
        let mut bones = serde_json::Map::new();
        for (b, m) in rig.bones.iter().zip(&pose) {
            bones.insert(b.name.clone(), matrix(*m));
        }
        let delta = rig.layer_delta(spine, &pose, &setup).unwrap();
        samples.push(json!({
            "anim": anim.map(|a| rig.animations[a].name.clone()),
            "t": t_ms as f64 / 1000.0,
            "bones": bones,
            "part": matrix(delta * place),
            "frame": qsketch_core::rig::frame_showing(doc, t_ms, true),
            "alpha": {
                "body": num(rig.alpha_at(anim.map(|a| (a, t_ms)), body)),
                "cloth": num(rig.alpha_at(anim.map(|a| (a, t_ms)), cloth)),
            },
            "mesh": rig.skin_points(&rig.meshes[0], &pose, &setup).iter().flat_map(|p| [num(p.x), num(p.y)]).collect::<Vec<_>>(),
        }));
    };
    push(None, 0);
    for (a, an) in rig.animations.iter().enumerate() {
        for t in [0, 50, 200, 399, 400, 450, 599, 600, 601, 700, 800, 900, 1000, 1100, 1199, 1200, 1500, 2500] {
            // Not the very end of a loop: a runtime wraps it to the start,
            // while the editor's playhead parked there shows the last key.
            if an.looped && t == an.duration_ms {
                continue;
            }
            push(Some(a), t);
        }
    }
    // Crossfades between the two animations (and out of the setup pose).
    let mut mixes = Vec::new();
    let name = |a: Option<usize>| a.map(|a| rig.animations[a].name.clone());
    for (from, from_ms, to, to_ms, weight) in [
        (Some(0), 300, Some(1), 0, 0.25f32),
        (Some(0), 400, Some(1), 100, 0.75),
        (Some(1), 650, Some(0), 50, 0.5),
        (Some(0), 1100, Some(1), 700, 0.1),
        (None, 0, Some(0), 500, 0.6),
    ] {
        let (f, t) = (from.map(|a| (a, from_ms)), to.map(|a| (a, to_ms)));
        let pose = rig.pose_mix(f, t, weight);
        let mut bones = serde_json::Map::new();
        for (b, m) in rig.bones.iter().zip(&pose) {
            bones.insert(b.name.clone(), matrix(*m));
        }
        mixes.push(json!({
            "from": name(from), "fromT": from_ms as f64 / 1000.0,
            "to": name(to), "toT": to_ms as f64 / 1000.0,
            "weight": weight,
            "bones": bones,
            "alpha": num(rig.alpha_mix(f, t, weight, cloth)),
        }));
    }
    serde_json::to_string_pretty(&json!({ "samples": samples, "mixes": mixes })).unwrap() + "\n"
}

// --- the sample character --------------------------------------------------------

/// One arm and one leg of the sample character, with where their parts go.
struct Limbs {
    upper: BoneId,
    thigh: BoneId,
    shin: BoneId,
    /// Left edges of the arm and leg parts.
    ax: i32,
    lx: i32,
    sleeve: Rgba8,
    trousers: Rgba8,
    shoe: Rgba8,
}

fn sample_doc() -> DocState {
    let mut doc = DocState::new(256, 256, None);
    doc.layers.clear();
    let skin = Rgba8::new(238, 196, 160, 255);
    let shirt = Rgba8::new(70, 120, 200, 255);
    let dark = Rgba8::new(60, 70, 110, 255);
    let shoe = Rgba8::new(60, 40, 40, 255);
    let far = |c: Rgba8| Rgba8::new(c.r / 2 + 20, c.g / 2 + 20, c.b / 2 + 20, 255);

    let root = doc.rig.add_bone("hips", None, 128.0, 150.0, 1.0);
    let torso = doc.rig.add_bone("torso", Some(root), 0.0, 0.0, 56.0);
    doc.rig.bone_mut(torso).unwrap().rotation = -90.0;
    let head = doc.rig.add_bone("head", Some(torso), 56.0, 0.0, 26.0);
    // Local (x, y) of the torso is world (y, -x): its y axis points right.
    let limb = |doc: &mut DocState, side: f32, near: bool| {
        let n = if side < 0.0 { "left" } else { "right" };
        let upper = doc.rig.add_bone(format!("{n} arm"), Some(torso), 46.0, 22.0 * side, 26.0);
        doc.rig.bone_mut(upper).unwrap().rotation = 180.0;
        doc.rig.add_bone(format!("{n} forearm"), Some(upper), 26.0, 0.0, 24.0);
        let thigh = doc.rig.add_bone(format!("{n} thigh"), Some(root), 14.0 * side, 0.0, 38.0);
        doc.rig.bone_mut(thigh).unwrap().rotation = 80.0;
        let shin = doc.rig.add_bone(format!("{n} shin"), Some(thigh), 38.0, 0.0, 36.0);
        doc.rig.bone_mut(shin).unwrap().rotation = 12.0;
        let (sh, sk, ss) = if near { (shirt, dark, shoe) } else { (far(shirt), far(dark), far(shoe)) };
        let ax = 128 + (22.0 * side) as i32 - 5;
        let lx = 128 + (14.0 * side) as i32 - 6;
        Limbs { upper, thigh, shin, ax, lx, sleeve: sh, trousers: sk, shoe: ss }
    };
    let left = limb(&mut doc, -1.0, false);
    let right = limb(&mut doc, 1.0, true);

    // Draw order, bottom to top: far limbs, torso, head, near limbs.
    {
        let Limbs { upper, thigh, shin, ax, lx, sleeve: sh, trousers: sk, shoe: ss, .. } = left;
        add_parts(&mut doc, "far leg", &[(IRect::new(lx, 150, 12, 74), sk)], thigh, Some(8));
        add_part(&mut doc, "far foot", IRect::new(lx - 4, 218, 26, 8), ss, shin);
        let arm = [(IRect::new(ax, 102, 10, 28), sh), (IRect::new(ax, 130, 10, 26), skin)];
        add_parts(&mut doc, "far arm", &arm, upper, Some(8));
    }
    add_part(&mut doc, "torso", IRect::new(108, 96, 40, 60), shirt, torso);
    add_part(&mut doc, "head", IRect::new(112, 56, 32, 40), skin, head);
    add_part(&mut doc, "eye", IRect::new(133, 70, 5, 5), Rgba8::new(30, 30, 40, 255), head);
    {
        let Limbs { upper, thigh, shin, ax, lx, sleeve: sh, trousers: sk, shoe: ss, .. } = right;
        add_parts(&mut doc, "leg", &[(IRect::new(lx, 150, 12, 74), sk)], thigh, Some(8));
        add_part(&mut doc, "foot", IRect::new(lx - 4, 218, 26, 8), ss, shin);
        let arm = [(IRect::new(ax, 102, 10, 28), sh), (IRect::new(ax, 130, 10, 26), skin)];
        add_parts(&mut doc, "arm", &arm, upper, Some(8));
    }

    let bone = |doc: &DocState, name: &str| doc.rig.bones.iter().find(|b| b.name == name).unwrap().id;
    let (lshin, rshin) = (bone(&doc, "left shin"), bone(&doc, "right shin"));
    let (_, lfoot) = doc.rig.add_ik(lshin).unwrap();
    let (_, rfoot) = doc.rig.add_ik(rshin).unwrap();
    for (id, n) in [(lfoot, "left foot"), (rfoot, "right foot")] {
        doc.rig.bone_mut(id).unwrap().name = n.into();
    }
    for c in &mut doc.rig.ik {
        c.name = if c.target == lfoot { "left leg" } else { "right leg" }.into();
    }
    let (lupper, lfore) = (bone(&doc, "left arm"), bone(&doc, "left forearm"));
    let (rupper, rfore) = (bone(&doc, "right arm"), bone(&doc, "right forearm"));

    // idle
    let a = doc.rig.add_animation("idle", 2000);
    mov(&mut doc, a, root, &[(0, 0.0, 0.0, EASE), (1000, 0.0, 3.0, EASE), (2000, 0.0, 0.0, EASE)]);
    rot(&mut doc, a, torso, &[(0, 0.0, EASE), (1000, 2.0, EASE), (2000, 0.0, EASE)]);
    rot(&mut doc, a, head, &[(0, 0.0, EASE), (1100, -4.0, EASE), (2000, 0.0, EASE)]);
    for arm in [lupper, rupper] {
        rot(&mut doc, a, arm, &[(0, 0.0, EASE), (1000, 3.0, EASE), (2000, 0.0, EASE)]);
    }

    // walk
    let w = doc.rig.add_animation("walk", 1000);
    mov(
        &mut doc,
        w,
        lfoot,
        &[
            (0, 24.0, 0.0, Curve::Linear),
            (500, -24.0, 0.0, EASE),
            (750, 0.0, -18.0, EASE),
            (1000, 24.0, 0.0, Curve::Linear),
        ],
    );
    mov(
        &mut doc,
        w,
        rfoot,
        &[
            (0, -24.0, 0.0, Curve::Linear),
            (250, 0.0, -18.0, EASE),
            (500, 24.0, 0.0, Curve::Linear),
            (1000, -24.0, 0.0, Curve::Linear),
        ],
    );
    mov(
        &mut doc,
        w,
        root,
        &[
            (0, 0.0, 2.0, EASE),
            (250, 0.0, -3.0, EASE),
            (500, 0.0, 2.0, EASE),
            (750, 0.0, -3.0, EASE),
            (1000, 0.0, 2.0, EASE),
        ],
    );
    rot(&mut doc, w, torso, &[(0, 3.0, EASE), (500, 3.0, EASE), (1000, 3.0, EASE)]);
    rot(&mut doc, w, lupper, &[(0, 30.0, EASE), (500, -30.0, EASE), (1000, 30.0, EASE)]);
    rot(&mut doc, w, rupper, &[(0, -30.0, EASE), (500, 30.0, EASE), (1000, -30.0, EASE)]);
    for arm in [lfore, rfore] {
        rot(&mut doc, w, arm, &[(0, -12.0, Curve::Linear), (1000, -12.0, Curve::Linear)]);
    }
    rot(&mut doc, w, head, &[(0, -2.0, EASE), (500, 2.0, EASE), (1000, -2.0, EASE)]);

    // wave (plays once)
    let v = doc.rig.add_animation("wave", 1200);
    doc.rig.animations[v].looped = false;
    rot(
        &mut doc,
        v,
        rupper,
        &[(0, 0.0, EASE), (250, -150.0, Curve::Linear), (1000, -150.0, EASE), (1200, 0.0, Curve::Linear)],
    );
    rot(
        &mut doc,
        v,
        rfore,
        &[
            (250, 0.0, EASE),
            (400, 28.0, EASE),
            (550, -28.0, EASE),
            (700, 28.0, EASE),
            (850, -28.0, EASE),
            (1000, 0.0, EASE),
        ],
    );
    rot(&mut doc, v, head, &[(0, 0.0, EASE), (250, 6.0, EASE), (1000, 6.0, EASE), (1200, 0.0, EASE)]);
    doc
}

// --- generate / check ------------------------------------------------------------

fn exported_rig_json(doc: &DocState) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    rig_io::export(doc, dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join("rig.json")).unwrap();
    (dir, text)
}

fn check(path: &Path, want: &str, update: bool) {
    if update {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        have.trim_end() == want.trim_end(),
        "{} is out of date: regenerate with\n  QSKETCH_UPDATE_PLAYER=1 cargo test -p qsketch-core --test rig_player_assets",
        path.display()
    );
}

#[test]
fn the_player_assets_are_up_to_date() {
    let update = std::env::var_os("QSKETCH_UPDATE_PLAYER").is_some();
    let base = player_dir();

    let (doc, spine) = fixture_doc();
    let (_keep, rig_json) = exported_rig_json(&doc);
    check(&base.join("fixture/rig.json"), &rig_json, update);
    check(&base.join("fixture/expected.json"), &fixture_expected(&doc, spine), update);

    let doc = sample_doc();
    let (dir, rig_json) = exported_rig_json(&doc);
    check(&base.join("sample/rig.json"), &rig_json, update);
    if update {
        let parts = base.join("sample/parts");
        let _ = std::fs::remove_dir_all(&parts);
        std::fs::create_dir_all(&parts).unwrap();
        for e in std::fs::read_dir(dir.path().join("parts")).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), parts.join(e.file_name())).unwrap();
        }
    }
}

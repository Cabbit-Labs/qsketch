//! The curve graph of the Rig panel: the selected bone's keyed values over
//! time (turn, move, scale, plus its IK mix and the active layer's opacity),
//! on the same time axis as the dopesheet above it. Every stretch between
//! two keys has two handles to shape how the value gets from one to the
//! other, and a row of presets sets the same thing in one click.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};
use qsketch_core::rig::{BoneId, Curve, RigAnim};
use qsketch_core::LayerId;

use crate::dialogs::rig::Easing;

const HEIGHT: f32 = 160.0;
/// A straight line, written as a Bézier: where the handles of a linear
/// stretch start.
const STRAIGHT: [f32; 4] = [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0];

/// One keyed timeline (a key list with its own easing curves).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tl {
    Rotate,
    Translate,
    Scale,
    Ik,
    Alpha,
}

impl Tl {
    const ALL: [Tl; 5] = [Tl::Rotate, Tl::Translate, Tl::Scale, Tl::Ik, Tl::Alpha];

    fn name(self) -> &'static str {
        match self {
            Tl::Rotate => "turn",
            Tl::Translate => "move",
            Tl::Scale => "scale",
            Tl::Ik => "IK mix",
            Tl::Alpha => "opacity",
        }
    }

    /// How many values a key holds (x and y share one curve).
    fn comps(self) -> usize {
        match self {
            Tl::Translate | Tl::Scale => 2,
            _ => 1,
        }
    }
}

/// The lines the graph can draw: timeline, component, name, color, unit.
const LINES: [(Tl, usize, &str, [u8; 3], &str); 7] = [
    (Tl::Rotate, 0, "turn", [240, 200, 60], "°"),
    (Tl::Translate, 0, "move x", [240, 110, 110], " px"),
    (Tl::Translate, 1, "move y", [110, 210, 110], " px"),
    (Tl::Scale, 0, "scale x", [100, 170, 250], ""),
    (Tl::Scale, 1, "scale y", [200, 140, 250], ""),
    (Tl::Ik, 0, "IK mix", [90, 220, 200], ""),
    (Tl::Alpha, 0, "opacity", [255, 160, 80], ""),
];

/// Whose timelines the graph shows.
pub struct Subject {
    pub bone: Option<BoneId>,
    pub ik: Option<u32>,
    pub layer: LayerId,
}

/// A key, whatever its timeline: one or two values.
#[derive(Clone, Copy)]
struct K {
    t: u32,
    v: [f32; 2],
    curve: Curve,
}

fn keys(anim: &RigAnim, s: &Subject, tl: Tl) -> Vec<K> {
    let track = s.bone.and_then(|b| anim.tracks.iter().find(|t| t.bone == b));
    let pair = |ks: &[qsketch_core::rig::Key<[f32; 2]>]| {
        ks.iter().map(|k| K { t: k.time_ms, v: k.value, curve: k.curve }).collect::<Vec<K>>()
    };
    let one = |ks: &[qsketch_core::rig::Key<f32>]| {
        ks.iter().map(|k| K { t: k.time_ms, v: [k.value, 0.0], curve: k.curve }).collect::<Vec<K>>()
    };
    match tl {
        Tl::Rotate => track.map(|t| one(&t.rotate)),
        Tl::Translate => track.map(|t| pair(&t.translate)),
        Tl::Scale => track.map(|t| pair(&t.scale)),
        Tl::Ik => {
            s.ik.and_then(|id| anim.ik.iter().find(|t| t.constraint == id))
                .map(|t| t.keys.iter().map(|k| K { t: k.time_ms, v: [k.value.mix, 0.0], curve: k.curve }).collect())
        }
        Tl::Alpha => anim.alpha.iter().find(|t| t.layer == s.layer).map(|t| one(&t.keys)),
    }
    .unwrap_or_default()
}

/// Every key's curve of one timeline, with its time.
fn curves_mut<'a>(anim: &'a mut RigAnim, s: &Subject, tl: Tl) -> Vec<(u32, &'a mut Curve)> {
    match tl {
        Tl::Rotate | Tl::Translate | Tl::Scale => {
            let Some(track) = s.bone.and_then(|b| anim.tracks.iter_mut().find(|t| t.bone == b)) else {
                return Vec::new();
            };
            match tl {
                Tl::Rotate => track.rotate.iter_mut().map(|k| (k.time_ms, &mut k.curve)).collect(),
                Tl::Translate => track.translate.iter_mut().map(|k| (k.time_ms, &mut k.curve)).collect(),
                _ => track.scale.iter_mut().map(|k| (k.time_ms, &mut k.curve)).collect(),
            }
        }
        Tl::Ik => {
            s.ik.and_then(|id| anim.ik.iter_mut().find(|t| t.constraint == id))
                .map(|t| t.keys.iter_mut().map(|k| (k.time_ms, &mut k.curve)).collect())
                .unwrap_or_default()
        }
        Tl::Alpha => anim
            .alpha
            .iter_mut()
            .find(|t| t.layer == s.layer)
            .map(|t| t.keys.iter_mut().map(|k| (k.time_ms, &mut k.curve)).collect())
            .unwrap_or_default(),
    }
}

fn set_curve(anim: &mut RigAnim, s: &Subject, tl: Tl, time_ms: u32, curve: Curve) {
    if let Some((_, c)) = curves_mut(anim, s, tl).into_iter().find(|(t, _)| *t == time_ms) {
        *c = curve;
    }
}

/// A line as (time, value) points: straight stretches before the first key
/// and after the last, and each stretch between keys along its curve.
fn polyline(ks: &[K], comp: usize, duration_ms: u32) -> Vec<(f32, f32)> {
    let mut pts = Vec::new();
    let (Some(first), Some(last)) = (ks.first(), ks.last()) else { return pts };
    pts.push((0.0, first.v[comp]));
    for w in ks.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ta, tb) = (a.t as f32, b.t as f32);
        let (va, vb) = (a.v[comp], b.v[comp]);
        match a.curve {
            Curve::Linear => pts.extend([(ta, va), (tb, vb)]),
            Curve::Stepped => pts.extend([(ta, va), (tb, va), (tb, vb)]),
            Curve::Bezier(_) => {
                for i in 0..=40 {
                    let s = i as f32 / 40.0;
                    pts.push((ta + (tb - ta) * s, va + (vb - va) * a.curve.ease(s)));
                }
            }
        }
    }
    pts.push((last.t as f32, last.v[comp]));
    pts.push((duration_ms.max(last.t) as f32, last.v[comp]));
    pts
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 <= f32::EPSILON { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    (p - (a + ab * t)).length()
}

fn fmt(v: f32) -> String {
    let s = if v.abs() >= 100.0 { format!("{v:.0}") } else { format!("{v:.2}") };
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

/// What the graph remembers between frames.
#[derive(Clone, Copy, Default)]
struct GraphState {
    /// The stretch being shaped: its timeline and the time of the key it
    /// starts at.
    seg: Option<(Tl, u32)>,
    /// The playhead when `seg` was picked: moving it picks the stretch under
    /// it again.
    at: u32,
    /// Hidden lines, one bit per entry of `LINES`.
    hidden: u8,
}

/// A handle being dragged. The value range is frozen for the drag so the
/// graph doesn't rescale under the pointer.
#[derive(Clone, Copy)]
struct HandleDrag {
    tl: Tl,
    from: u32,
    comp: usize,
    second: bool,
    lo: f32,
    hi: f32,
}

/// Draws the curve controls and the graph, editing key curves of `anim` in
/// place. Returns a time when the playhead was scrubbed.
pub fn ui(ui: &mut Ui, anim: &mut RigAnim, subject: &Subject, time_ms: u32, gutter: f32, step: u32) -> Option<u32> {
    let state_id = ui.id().with("rig_graph_state");
    let drag_id = ui.id().with("rig_graph_drag");
    let mut gs: GraphState = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
    let drag: Option<HandleDrag> = ui.data(|d| d.get_temp(drag_id));
    let all: Vec<Vec<K>> = Tl::ALL.iter().map(|&tl| keys(anim, subject, tl)).collect();
    let of = |tl: Tl| &all[Tl::ALL.iter().position(|&x| x == tl).unwrap_or(0)];

    // The stretch to shape: the picked one while the playhead stays put,
    // otherwise the one under the playhead (of the same timeline if it can be).
    let valid = |(tl, from): (Tl, u32)| of(tl).iter().position(|k| k.t == from).is_some_and(|i| i + 1 < of(tl).len());
    if drag.is_none() && (gs.at != time_ms || !gs.seg.is_some_and(valid)) {
        let pick = |tl: Tl| {
            let ks = of(tl);
            (ks.len() >= 2).then(|| {
                let i = ks.partition_point(|k| k.t <= time_ms).saturating_sub(1).min(ks.len() - 2);
                (tl, ks[i].t)
            })
        };
        // The timeline picked before; else the first one that goes somewhere
        // (a turn held at 0 has nothing to shape); else any with two keys.
        let moves = |tl: Tl| of(tl).windows(2).any(|w| w[0].v != w[1].v);
        gs.seg = gs
            .seg
            .and_then(|s| pick(s.0))
            .or_else(|| Tl::ALL.into_iter().filter(|&tl| moves(tl)).find_map(pick))
            .or_else(|| Tl::ALL.into_iter().find_map(pick));
        gs.at = time_ms;
    }
    let seg_keys = gs.seg.and_then(|(tl, from)| {
        let ks = of(tl);
        let i = ks.iter().position(|k| k.t == from)?;
        Some((tl, ks[i], *ks.get(i + 1)?))
    });

    // --- the controls: presets and numbers for the picked stretch ------------
    let mut new_curve: Option<Curve> = None;
    let mut to_all = false;
    ui.horizontal_wrapped(|ui| {
        ui.strong("Curve");
        match seg_keys {
            Some((tl, a, b)) => {
                ui.label(format!("{} {}–{} ms", tl.name(), a.t, b.t));
                for e in Easing::ALL {
                    if ui.selectable_label(a.curve == e.curve(), e.label()).clicked() {
                        new_curve = Some(e.curve());
                    }
                }
                if let Curve::Bezier(mut c) = a.curve {
                    ui.separator();
                    let mut changed = false;
                    for (i, name) in ["x1 ", "y1 ", "x2 ", "y2 "].into_iter().enumerate() {
                        let range = if i % 2 == 0 { 0.0..=1.0 } else { -2.0..=3.0 };
                        let w = egui::DragValue::new(&mut c[i]).speed(0.01).range(range).prefix(name).max_decimals(2);
                        changed |= ui.add_sized([62.0, 20.0], w).changed();
                    }
                    if changed {
                        new_curve = Some(Curve::Bezier(c));
                    }
                }
                ui.separator();
                to_all = ui
                    .button("Apply to all")
                    .on_hover_text("Give every key of the curves shown below this easing")
                    .clicked();
            }
            None => {
                ui.weak("Key the selected bone at two times, then shape how it gets from one to the other here.");
            }
        }
    });
    if let (Some(c), Some((tl, a, _))) = (new_curve, seg_keys) {
        set_curve(anim, subject, tl, a.t, c);
    }
    if let (true, Some((_, a, _))) = (to_all, seg_keys) {
        let c = new_curve.unwrap_or(a.curve);
        for tl in Tl::ALL {
            for (_, slot) in curves_mut(anim, subject, tl) {
                *slot = c;
            }
        }
    }

    // --- the graph -------------------------------------------------------------
    let width = ui.available_width().max(gutter + 160.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    let (accent, text, weak) = (visuals.selection.bg_fill, visuals.text_color(), visuals.weak_text_color());
    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let (x0, x1) = (rect.left() + gutter, rect.right() - 14.0);
    let (y0, y1) = (rect.top() + 14.0, rect.bottom() - 14.0);
    let dur = anim.duration_ms.max(1) as f32;
    let to_x = |t: f32| x0 + (x1 - x0) * t / dur;
    let to_t = |x: f32| (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) * dur).round() as u32;

    // The value range of each timeline (x and y share one): as keyed, and
    // with some air around it for drawing.
    let raw: Vec<(f32, f32)> = Tl::ALL
        .iter()
        .map(|&tl| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for comp in 0..tl.comps() {
                for k in of(tl) {
                    lo = lo.min(k.v[comp]);
                    hi = hi.max(k.v[comp]);
                }
            }
            if lo > hi {
                (0.0, 1.0)
            } else {
                (lo, hi)
            }
        })
        .collect();
    let ranges: Vec<(f32, f32)> = Tl::ALL
        .iter()
        .enumerate()
        .map(|(n, &tl)| {
            if let Some(d) = drag.filter(|d| d.tl == tl) {
                return (d.lo, d.hi);
            }
            // Curves may overshoot their keys.
            let (mut lo, mut hi) = raw[n];
            for comp in 0..tl.comps() {
                for (_, v) in polyline(of(tl), comp, anim.duration_ms) {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            // The handles of the picked stretch stay in reach too.
            if let Some((_, a, b)) = seg_keys.filter(|s| s.0 == tl) {
                if let Curve::Bezier(c) = a.curve {
                    for comp in 0..tl.comps() {
                        let dv = b.v[comp] - a.v[comp];
                        for y in [c[1], c[3]] {
                            lo = lo.min(a.v[comp] + y * dv);
                            hi = hi.max(a.v[comp] + y * dv);
                        }
                    }
                }
            }
            if hi - lo < 1e-4 {
                let pad = (lo.abs() * 0.25).max(0.5);
                return (lo - pad, hi + pad);
            }
            let pad = (hi - lo) * 0.12;
            (lo - pad, hi + pad)
        })
        .collect();
    let range = |tl: Tl| ranges[Tl::ALL.iter().position(|&x| x == tl).unwrap_or(0)];
    let to_y = |tl: Tl, v: f32| {
        let (lo, hi) = range(tl);
        y1 - (y1 - y0) * (v - lo) / (hi - lo)
    };
    let to_v = |tl: Tl, y: f32| {
        let (lo, hi) = range(tl);
        lo + (hi - lo) * (y1 - y) / (y1 - y0)
    };
    let tint = |rgb: [u8; 3]| {
        // Darker on a light theme, where the bright colors wash out.
        let m = if visuals.dark_mode { 1.0 } else { 0.62 };
        Color32::from_rgb((rgb[0] as f32 * m) as u8, (rgb[1] as f32 * m) as u8, (rgb[2] as f32 * m) as u8)
    };

    // Time grid.
    let grid = weak.gamma_multiply(0.18);
    let mut t = 0u32;
    while t <= anim.duration_ms {
        let x = to_x(t as f32);
        painter
            .line_segment([Pos2::new(x, rect.top() + 2.0), Pos2::new(x, rect.bottom() - 2.0)], Stroke::new(1.0, grid));
        let label = if t % 1000 == 0 { format!("{}s", t / 1000) } else { format!("{t}") };
        painter.text(Pos2::new(x + 3.0, rect.top() + 2.0), Align2::LEFT_TOP, label, FontId::proportional(10.0), weak);
        t += step.max(1);
    }
    painter.line_segment([Pos2::new(x0, y0), Pos2::new(x1, y0)], Stroke::new(1.0, grid));
    painter.line_segment([Pos2::new(x0, y1), Pos2::new(x1, y1)], Stroke::new(1.0, grid));

    // The legend, in the name column: a line per curve with its range.
    // Clicking one hides or shows its curve.
    let shown: Vec<usize> = (0..LINES.len()).filter(|&i| !of(LINES[i].0).is_empty()).collect();
    let legend_y = |n: usize| rect.top() + 14.0 + 20.0 * n as f32;
    let names = painter.with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(x0 - 8.0, rect.bottom())));
    let font = FontId::proportional(11.0);
    for (n, &i) in shown.iter().enumerate() {
        let (tl, _, label, rgb, unit) = LINES[i];
        let hidden = gs.hidden & (1 << i) != 0;
        let y = legend_y(n);
        let col = if hidden { weak.gamma_multiply(0.5) } else { tint(rgb) };
        names.line_segment([Pos2::new(rect.left() + 8.0, y), Pos2::new(rect.left() + 22.0, y)], Stroke::new(3.0, col));
        // This line's own range (x and y of a move are scaled together, but
        // each says how far it goes itself).
        let comp = LINES[i].1;
        let (lo, hi) = of(tl).iter().fold((f32::MAX, f32::MIN), |(lo, hi), k| (lo.min(k.v[comp]), hi.max(k.v[comp])));
        let label = if hidden {
            format!("{label} (hidden)")
        } else if hi - lo < 1e-4 {
            format!("{label}  {}{unit}", fmt(lo))
        } else {
            format!("{label}  {}…{}{unit}", fmt(lo), fmt(hi))
        };
        let label_col = if hidden { weak } else { text };
        names.text(Pos2::new(rect.left() + 28.0, y), Align2::LEFT_CENTER, label, font.clone(), label_col);
    }
    if shown.is_empty() {
        painter.text(
            Pos2::new((x0 + x1) / 2.0, rect.center().y),
            Align2::CENTER_CENTER,
            "Nothing to graph: select a bone that has keys.",
            font.clone(),
            weak,
        );
    }

    // The curves and their keys.
    let plot = painter.with_clip_rect(Rect::from_min_max(Pos2::new(x0 - 7.0, rect.top()), rect.max));
    let mut hit: Vec<(Tl, Vec<Pos2>)> = Vec::new();
    for &i in &shown {
        let (tl, comp, _, rgb, _) = LINES[i];
        if gs.hidden & (1 << i) != 0 {
            continue;
        }
        let ks = of(tl);
        let pts: Vec<Pos2> =
            polyline(ks, comp, anim.duration_ms).into_iter().map(|(t, v)| Pos2::new(to_x(t), to_y(tl, v))).collect();
        let col = tint(rgb);
        plot.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, col)));
        for k in ks {
            let picked = gs.seg == Some((tl, k.t));
            let c = Pos2::new(to_x(k.t as f32), to_y(tl, k.v[comp]));
            plot.circle(c, if picked { 4.5 } else { 3.5 }, col, Stroke::new(1.0, Color32::BLACK));
        }
        hit.push((tl, pts));
    }

    // The handles of the picked stretch, on each of its lines that changes.
    // (handle position, component, is the second handle)
    let mut handles: Vec<(Pos2, usize, bool)> = Vec::new();
    if let Some((tl, a, b)) = seg_keys {
        let c = match a.curve {
            Curve::Bezier(c) => Some(c),
            Curve::Linear => Some(STRAIGHT),
            Curve::Stepped => None,
        };
        for &i in &shown {
            let (ltl, comp, _, rgb, _) = LINES[i];
            let dv = b.v[comp] - a.v[comp];
            if ltl != tl || gs.hidden & (1 << i) != 0 || dv.abs() < 1e-6 {
                continue;
            }
            let Some(c) = c else { continue };
            let dt = (b.t - a.t) as f32;
            let pa = Pos2::new(to_x(a.t as f32), to_y(tl, a.v[comp]));
            let pb = Pos2::new(to_x(b.t as f32), to_y(tl, b.v[comp]));
            let h1 = Pos2::new(to_x(a.t as f32 + c[0] * dt), to_y(tl, a.v[comp] + c[1] * dv));
            let h2 = Pos2::new(to_x(a.t as f32 + c[2] * dt), to_y(tl, a.v[comp] + c[3] * dv));
            let col = tint(rgb);
            for (key, h, second) in [(pa, h1, false), (pb, h2, true)] {
                plot.line_segment([key, h], Stroke::new(1.5, text.gamma_multiply(0.7)));
                plot.circle(h, 5.5, Color32::WHITE, Stroke::new(2.0, col));
                plot.circle_stroke(h, 6.5, Stroke::new(1.0, Color32::BLACK));
                handles.push((h, comp, second));
            }
        }
    }

    // Playhead.
    let px = to_x(time_ms.min(anim.duration_ms) as f32);
    painter
        .line_segment([Pos2::new(px, rect.top() + 2.0), Pos2::new(px, rect.bottom() - 2.0)], Stroke::new(2.0, accent));

    // --- interaction -------------------------------------------------------------
    let mut scrub = None;
    let in_names = |x: f32| x < x0 - 9.0;
    if resp.drag_started() {
        if let (Some(p), Some((tl, a, _))) = (ui.input(|i| i.pointer.press_origin()), seg_keys) {
            let nearest = handles
                .iter()
                .map(|&(h, comp, second)| ((h - p).length(), comp, second))
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .filter(|x| x.0 <= 10.0);
            if let Some((_, comp, second)) = nearest {
                let (lo, hi) = range(tl);
                ui.data_mut(|d| d.insert_temp(drag_id, HandleDrag { tl, from: a.t, comp, second, lo, hi }));
            }
        }
    }
    let drag: Option<HandleDrag> = ui.data(|d| d.get_temp(drag_id));
    if let Some(d) = drag {
        let ks = of(d.tl);
        let ends = ks.iter().position(|k| k.t == d.from).and_then(|i| Some((ks[i], *ks.get(i + 1)?)));
        if let (Some(p), Some((a, b))) = (resp.interact_pointer_pos(), ends) {
            let (xa, xb) = (to_x(a.t as f32), to_x(b.t as f32));
            let dv = b.v[d.comp] - a.v[d.comp];
            let cx = ((p.x - xa) / (xb - xa).max(1.0)).clamp(0.0, 1.0);
            let cy = ((to_v(d.tl, p.y) - a.v[d.comp]) / dv).clamp(-2.0, 3.0);
            let round = |v: f32| (v * 100.0).round() / 100.0;
            let mut c = match a.curve {
                Curve::Bezier(c) => c,
                _ => STRAIGHT,
            };
            let at = if d.second { 2 } else { 0 };
            (c[at], c[at + 1]) = (round(cx), round(cy));
            set_curve(anim, subject, d.tl, d.from, Curve::Bezier(c));
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
            ui.data_mut(|d| d.remove::<HandleDrag>(drag_id));
        }
    } else if let Some(p) = resp.interact_pointer_pos() {
        let from_names = ui.input(|i| i.pointer.press_origin()).is_some_and(|o| in_names(o.x));
        if resp.clicked() && in_names(p.x) {
            // The legend: hide or show a curve.
            if let Some(&i) = shown.iter().enumerate().find(|(n, _)| (legend_y(*n) - p.y).abs() <= 10.0).map(|(_, i)| i)
            {
                gs.hidden ^= 1 << i;
            }
        } else if !from_names {
            // A curve under the pointer: pick its stretch there. Anywhere
            // else: scrub, like the dopesheet.
            let on_curve = resp
                .clicked()
                .then(|| {
                    hit.iter()
                        .map(|(tl, pts)| {
                            (pts.windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f32::MAX, f32::min), *tl)
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .filter(|x| x.0 <= 7.0)
                })
                .flatten();
            if let Some((_, tl)) = on_curve {
                let ks = of(tl);
                if ks.len() >= 2 {
                    let t = to_t(p.x);
                    let i = ks.partition_point(|k| k.t <= t).saturating_sub(1).min(ks.len() - 2);
                    gs.seg = Some((tl, ks[i].t));
                    gs.at = time_ms;
                }
            } else if resp.dragged() || resp.clicked() {
                let snap = !ui.input(|i| i.modifiers.shift);
                scrub = Some(if snap { ((to_t(p.x) + 2) / 5 * 5).min(anim.duration_ms) } else { to_t(p.x) });
            }
        }
    }
    resp.on_hover_text(
        "Drag a handle to shape how the value gets from one key to the next. Click a curve to pick another stretch of it, or a name on the left to hide its curve. Click or drag anywhere else to scrub.",
    );
    ui.data_mut(|d| d.insert_temp(state_id, gs));
    scrub
}

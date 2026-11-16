/*
 * qSketch rig runtime: poses a `qsketch-rig` (version 1) skeleton.
 *
 * This file knows nothing about the DOM or any renderer. It is written from
 * docs/RIG_FORMAT.md alone, so it doubles as a template for a runtime in
 * another engine. Matrices are {a, b, c, d, e, f} like an SVG matrix():
 *   x' = a*x + c*y + e,   y' = b*x + d*y + f
 *
 * Usage:
 *   const rig = QRig.load(json);                     // parse once
 *   const world = QRig.pose(rig, "walk", seconds);   // one matrix per bone
 *   const m = QRig.partMatrix(rig, part, world);     // image pixels -> canvas
 *   QRig.meshPoints(rig, part, world)                // parts with a `mesh`: vertices
 *   QRig.pose(rig, null, 0)                          // the setup pose
 */
(function (root) {
  "use strict";

  const IDENTITY = Object.freeze({ a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 });

  /** `p * q`: apply q first, then p. */
  function mul(p, q) {
    return {
      a: p.a * q.a + p.c * q.b,
      b: p.b * q.a + p.d * q.b,
      c: p.a * q.c + p.c * q.d,
      d: p.b * q.c + p.d * q.d,
      e: p.a * q.e + p.c * q.f + p.e,
      f: p.b * q.e + p.d * q.f + p.f,
    };
  }

  function apply(m, x, y) {
    return { x: m.a * x + m.c * y + m.e, y: m.b * x + m.d * y + m.f };
  }

  /** translate(x, y) * rotate(deg, clockwise on screen) * scale(sx, sy) */
  function trs(x, y, deg, sx, sy) {
    const r = (deg * Math.PI) / 180;
    const s = Math.sin(r);
    const c = Math.cos(r);
    return { a: c * sx, b: s * sx, c: -s * sy, d: c * sy, e: x, f: y };
  }

  /** Degrees wrapped into -180..180. */
  function wrap180(deg) {
    return ((((deg + 540) % 360) + 360) % 360) - 180;
  }

  /** Eased progress for a key's `curve` ("linear", "stepped" or 4 numbers). */
  function ease(curve, t) {
    t = Math.min(1, Math.max(0, t));
    if (curve === "stepped") return 0;
    if (!Array.isArray(curve)) return t;
    const [x1, y1, x2, y2] = curve;
    const cx1 = Math.min(1, Math.max(0, x1));
    const cx2 = Math.min(1, Math.max(0, x2));
    const bez = (p, q, s) => {
      const u = 1 - s;
      return 3 * u * u * s * p + 3 * u * s * s * q + s * s * s;
    };
    let lo = 0;
    let hi = 1;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (bez(cx1, cx2, mid) < t) lo = mid;
      else hi = mid;
    }
    return bez(y1, y2, (lo + hi) / 2);
  }

  /**
   * Sample a timeline at `t` seconds: clamped before the first and after the
   * last key, otherwise interpolated with the EARLIER key's curve. `fields`
   * names the numeric properties of a key (e.g. ["x", "y"]); the result has
   * the same names. Returns null for an empty or missing timeline.
   */
  function sample(keys, t, fields) {
    if (!keys || keys.length === 0) return null;
    const pick = (k) => Object.fromEntries(fields.map((f) => [f, k[f]]));
    if (t <= keys[0].time) return pick(keys[0]);
    const last = keys[keys.length - 1];
    if (t >= last.time) return pick(last);
    let i = 1;
    while (keys[i].time <= t) i++;
    const a = keys[i - 1];
    const b = keys[i];
    const p = ease(a.curve, (t - a.time) / Math.max(b.time - a.time, 1e-9));
    return Object.fromEntries(fields.map((f) => [f, a[f] + (b[f] - a[f]) * p]));
  }

  /** The bend of the last key at or before `t` (bends are stepped). */
  function sampleBend(keys, t, fallback) {
    if (!keys || keys.length === 0) return fallback;
    let bend = keys[0].bend;
    for (const k of keys) if (k.time <= t) bend = k.bend;
    return bend;
  }

  /** Index the file's bones, IK and parts for fast posing. */
  function load(json) {
    if (json.format !== "qsketch-rig" || json.version !== 1) {
      throw new Error("not a qsketch-rig version 1 file");
    }
    const index = new Map(json.bones.map((b, i) => [b.name, i]));
    const bones = json.bones.map((b) => ({
      name: b.name,
      parent: b.parent == null ? -1 : index.get(b.parent),
      x: b.x,
      y: b.y,
      rotation: b.rotation,
      scaleX: b.scaleX,
      scaleY: b.scaleY,
      length: b.length,
    }));
    const ik = (json.ik || []).map((c) => ({
      name: c.name,
      parent: index.get(c.bones[0]),
      child: index.get(c.bones[1]),
      target: index.get(c.target),
      bend: c.bend,
      mix: c.mix,
    }));
    // Drives (gears): drive[i] is the bone that turns bone i, or null.
    const drive = bones.map(() => null);
    for (const d of json.drives || []) {
      drive[index.get(d.bone)] = { source: index.get(d.source), ratio: d.ratio };
    }
    return {
      json,
      canvas: json.canvas,
      bones,
      index,
      ik,
      drive,
      parts: json.parts,
      animations: json.animations || {},
    };
  }

  function worlds(bones, locals) {
    const out = new Array(bones.length);
    for (let i = 0; i < bones.length; i++) {
      const l = locals[i];
      const local = trs(l.x, l.y, l.rot, l.sx, l.sy);
      out[i] = bones[i].parent >= 0 ? mul(out[bones[i].parent], local) : local;
    }
    return out;
  }

  const angle = (m) => Math.atan2(m.b, m.a);

  /** Solve one two-bone constraint on `locals` (see "Two-bone IK" in the doc). */
  function solveIk(rig, locals, c, mix, bend) {
    if (mix <= 0) return;
    const bones = rig.bones;
    if (bones[c.child].parent !== c.parent) return;
    const w = worlds(bones, locals);
    const h = apply(w[c.parent], 0, 0);
    const hc = apply(w[c.child], 0, 0);
    const t = apply(w[c.target], 0, 0);
    const a = Math.hypot(hc.x - h.x, hc.y - h.y);
    const b = bones[c.child].length * Math.hypot(w[c.child].a, w[c.child].b);
    const dRaw = Math.hypot(t.x - h.x, t.y - h.y);
    if (a < 1e-4 || b < 1e-4 || dRaw < 1e-4) return;
    const d = Math.min(a + b, Math.max(Math.max(Math.abs(a - b), 1e-4), dRaw));
    const cosA = Math.min(1, Math.max(-1, (a * a + d * d - b * b) / (2 * a * d)));
    const alpha = Math.acos(cosA);
    const theta = Math.atan2(t.y - h.y, t.x - h.x) + (bend < 0 ? -alpha : alpha);
    const offset = Math.atan2(hc.y - h.y, hc.x - h.x) - angle(w[c.parent]);
    const pp = bones[c.parent].parent >= 0 ? angle(w[bones[c.parent].parent]) : 0;
    const rotP = ((theta - offset - pp) * 180) / Math.PI;
    const p = locals[c.parent];
    p.rot = p.rot + mix * wrap180(rotP - p.rot);

    const w2 = worlds(bones, locals);
    const hc2 = apply(w2[c.child], 0, 0);
    const rotC = ((Math.atan2(t.y - hc2.y, t.x - hc2.x) - angle(w2[c.parent])) * 180) / Math.PI;
    const ch = locals[c.child];
    ch.rot = ch.rot + mix * wrap180(rotC - ch.rot);
  }

  /** The time to sample `anim` at: wrapped when it loops, else held at the ends. */
  function loopTime(anim, seconds) {
    if (anim.loop) {
      // `%` is exact; adding the duration first would not be.
      const t = seconds % anim.duration;
      return t < 0 ? t + anim.duration : t;
    }
    return Math.min(Math.max(seconds, 0), anim.duration);
  }

  /**
   * World matrix of every bone. `animName` null (or unknown) gives the setup
   * pose, which is plain forward kinematics; IK applies only to animations.
   */
  function pose(rig, animName, seconds) {
    return worlds(rig.bones, locals(rig, animName, seconds));
  }

  /**
   * World matrices while crossfading from one animation to another (see
   * "Mixing" in the doc): `weight` 0 is all `from`, 1 all `to`. Either name
   * may be null for the setup pose. `rig.json.mix` is the crossfade length
   * (seconds) set in the editor.
   */
  function poseMix(rig, fromAnim, fromSeconds, toAnim, toSeconds, weight) {
    const w = Math.min(1, Math.max(0, weight));
    if (w >= 1) return pose(rig, toAnim, toSeconds);
    const a = locals(rig, fromAnim, fromSeconds);
    const b = locals(rig, toAnim, toSeconds);
    a.forEach((l, i) => {
      const o = b[i];
      l.x += (o.x - l.x) * w;
      l.y += (o.y - l.y) * w;
      l.rot += wrap180(o.rot - l.rot) * w;
      l.sx += (o.sx - l.sx) * w;
      l.sy += (o.sy - l.sy) * w;
    });
    return worlds(rig.bones, a);
  }

  /** How shown `part` is during the same crossfade. */
  function partAlphaMix(rig, part, fromAnim, fromSeconds, toAnim, toSeconds, weight) {
    const w = Math.min(1, Math.max(0, weight));
    const a = partAlpha(rig, part, fromAnim, fromSeconds);
    return a + (partAlpha(rig, part, toAnim, toSeconds) - a) * w;
  }

  /** Every bone's local transform in an animation, after drives and IK. */
  function locals(rig, animName, seconds) {
    const anim = animName != null ? rig.animations[animName] : null;
    const t = anim ? loopTime(anim, seconds) : seconds;
    const locals = rig.bones.map((b) => {
      const l = { x: b.x, y: b.y, rot: b.rotation, sx: b.scaleX, sy: b.scaleY };
      const tr = anim && anim.bones[b.name];
      if (tr) {
        const tl = sample(tr.translate, t, ["x", "y"]);
        if (tl) {
          l.x += tl.x;
          l.y += tl.y;
        }
        const ro = sample(tr.rotate, t, ["angle"]);
        if (ro) l.rot += ro.angle;
        const sc = sample(tr.scale, t, ["x", "y"]);
        if (sc) {
          l.sx *= sc.x;
          l.sy *= sc.y;
        }
      }
      return l;
    });
    if (anim && rig.drive.some((d) => d)) {
      // A driven bone also turns by `ratio` times what its source is turned
      // by: the source's own rotate keys plus the source's drive.
      const keyed = rig.bones.map((b) => {
        const tr = anim.bones[b.name];
        const ro = tr && sample(tr.rotate, t, ["angle"]);
        return ro ? ro.angle : 0;
      });
      rig.bones.forEach((_, i) => {
        let factor = 1;
        for (let d = rig.drive[i], n = 0; d && n < rig.bones.length; d = rig.drive[d.source], n++) {
          factor *= d.ratio;
          locals[i].rot += factor * keyed[d.source];
        }
      });
    }
    if (anim) {
      for (const c of rig.ik) {
        const keys = anim.ik && anim.ik[c.name];
        const v = sample(keys, t, ["mix"]);
        const mix = Math.min(1, Math.max(0, v ? v.mix : c.mix));
        solveIk(rig, locals, c, mix, sampleBend(keys, t, c.bend));
      }
    }
    return locals;
  }

  function inverse(m) {
    const det = m.a * m.d - m.b * m.c;
    const a = m.d / det;
    const b = -m.b / det;
    const c = -m.c / det;
    const d = m.a / det;
    return { a, b, c, d, e: -(a * m.e + c * m.f), f: -(b * m.e + d * m.f) };
  }

  /**
   * Where a meshed part's vertices are under `world` (see "Meshes" in the
   * doc): a Float64Array of x, y pairs in canvas space, row-major with
   * (cols + 1) vertices per row. Each vertex blends how far its bones have
   * moved from the setup pose.
   */
  function meshPoints(rig, part, world) {
    const mesh = part.mesh;
    if (!rig.setupInverse) {
      rig.setupInverse = pose(rig, null, 0).map(inverse);
    }
    const deltas = mesh.bones.map((name) => {
      const i = rig.index.get(name);
      return mul(world[i], rig.setupInverse[i]);
    });
    const stride = mesh.cols + 1;
    const out = new Float64Array(mesh.vertices.length * 2);
    mesh.vertices.forEach((influences, v) => {
      const px = part.x + (part.width * (v % stride)) / mesh.cols;
      const py = part.y + (part.height * Math.floor(v / stride)) / mesh.rows;
      let x = 0;
      let y = 0;
      let sum = 0;
      for (let k = 0; k < influences.length; k += 2) {
        const q = apply(deltas[influences[k]], px, py);
        x += q.x * influences[k + 1];
        y += q.y * influences[k + 1];
        sum += influences[k + 1];
      }
      out[v * 2] = sum > 0 ? x / sum : px;
      out[v * 2 + 1] = sum > 0 ? y / sum : py;
    });
    return out;
  }

  /** The mesh's triangles as [i0, i1, i2] vertex indices, two per cell. */
  function meshTriangles(mesh) {
    const tris = [];
    const stride = mesh.cols + 1;
    for (let j = 0; j < mesh.rows; j++) {
      for (let i = 0; i < mesh.cols; i++) {
        const v00 = j * stride + i;
        const v10 = v00 + 1;
        const v01 = v00 + stride;
        const v11 = v01 + 1;
        tris.push([v00, v10, v01], [v10, v11, v01]);
      }
    }
    return tris;
  }

  /**
   * How shown `part` is (0..1): its own timeline in `animName` at `seconds`
   * when it has one, else its rest `alpha` (1 when absent). Multiply it into
   * the part's `opacity`.
   */
  function partAlpha(rig, part, animName, seconds) {
    const rest = part.alpha == null ? 1 : part.alpha;
    const anim = animName != null ? rig.animations[animName] : null;
    const keys = anim && anim.parts && anim.parts[part.name];
    if (!keys || keys.length === 0) return rest;
    return Math.min(1, Math.max(0, sample(keys, loopTime(anim, seconds), ["alpha"]).alpha));
  }

  /**
   * Whether `part` is drawn while the skin named `skin` is worn (see "Skins"
   * in the doc): a part in no skin always is; with `skin` null every part is.
   */
  function partInSkin(part, skin) {
    return skin == null || !part.skins || part.skins.length === 0 || part.skins.includes(skin);
  }

  /**
   * The document frame showing at `seconds` (see "Frame art" in the doc):
   * the frames cycle, in whole milliseconds. 0 when the file has no frames.
   */
  function frameAt(rig, seconds) {
    const frames = rig.json.frames;
    if (!frames || frames.length === 0) return 0;
    const ms = frames.map((s) => Math.round(s * 1000));
    const total = ms.reduce((a, b) => a + b, 0);
    if (total <= 0) return 0;
    let t = Math.round(seconds * 1000) % total;
    if (t < 0) t += total;
    for (let i = 0; i < ms.length; i++) {
      if (t < ms[i]) return i;
      t -= ms[i];
    }
    return ms.length - 1;
  }

  /** The image path `part` draws: its frame art while an animation plays. */
  function partImage(rig, part, animName, seconds) {
    if (animName == null || !part.images || part.images.length === 0) return part.image;
    return part.images[frameAt(rig, seconds)] || part.image;
  }

  /** The matrix that draws `part`'s image (pixel 0,0 at its top-left). */
  function partMatrix(rig, part, world) {
    const [a, b, c, d, e, f] = part.transform;
    const m = { a, b, c, d, e, f };
    if (part.bone == null) return m;
    return mul(world[rig.index.get(part.bone)], m);
  }

  /** Canvas 2D `globalCompositeOperation` for an exported blend name. */
  const BLEND = {
    normal: "source-over",
    passthrough: "source-over",
    darken: "darken",
    multiply: "multiply",
    colorburn: "color-burn",
    lighten: "lighten",
    screen: "screen",
    colordodge: "color-dodge",
    lineardodge: "lighter",
    overlay: "overlay",
    softlight: "soft-light",
    hardlight: "hard-light",
    difference: "difference",
    exclusion: "exclusion",
    hue: "hue",
    saturation: "saturation",
    color: "color",
    luminosity: "luminosity",
  };
  const canvasBlend = (name) => BLEND[name] || "source-over";

  const QRig = {
    IDENTITY, mul, apply, inverse, trs, wrap180, ease, sample, load, pose, poseMix, partMatrix, partAlpha, partAlphaMix, partInSkin, frameAt, partImage, meshPoints, meshTriangles, canvasBlend,
  };
  if (typeof module !== "undefined" && module.exports) module.exports = QRig;
  else root.QRig = QRig;
})(typeof self !== "undefined" ? self : this);

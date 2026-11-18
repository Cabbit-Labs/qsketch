//! Node animation: a file's nodes (their names, parents and rest transforms) and its animations (each node's translation, rotation and
//! scale over time), read from glTF, and the world transform of every node at a moment of an animation. Rigid: a mesh moves with its
//! node, as the parts of a console character do on their bones. Skins (vertices weighted to several joints) are not read.
//!
//! Used by `Mesh::posed`, which moves each piece of a mesh from where its node stands at rest to where it stands at that moment (Resin,
//! `/code/resin`, plays recorded moves this way); qSketch's Model panel shows the rest pose.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::camera::{mul, Mat4, IDENTITY};

/// A node's transform: translation, rotation (a unit quaternion, `[x, y, z, w]`) and scale; or a matrix, when the file gives one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Local {
    Trs { t: [f32; 3], r: [f32; 4], s: [f32; 3] },
    Matrix(Mat4),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub rest: Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Translation,
    Rotation,
    Scale,
}

/// One node's one property over time: its keys' times (seconds) and values (three numbers, or four for a rotation).
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub path: Path,
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
    /// Holds each key's value until the next (`STEP`); else between keys it moves in a line (a rotation along the sphere).
    pub step: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
    /// Its last key's time.
    pub duration: f32,
    /// The animation's `extras`, as the file has them (Resin keeps a recording's volumes there).
    pub extras: Option<Value>,
}

impl Local {
    pub fn matrix(&self) -> Mat4 {
        match *self {
            Local::Matrix(m) => m,
            Local::Trs { t, r, s } => trs_matrix(t, r, s),
        }
    }
}

/// The world transform of every node, at rest, or at `time` seconds into `animation` (its channels override the rest transform's parts).
pub fn world(nodes: &[Node], animation: Option<(&Animation, f32)>) -> Vec<Mat4> {
    let mut local: Vec<Local> = nodes.iter().map(|n| n.rest).collect();
    if let Some((a, time)) = animation {
        for c in &a.channels {
            let Some(l) = local.get_mut(c.node) else { continue };
            let (mut t, mut r, mut s) = match *l {
                Local::Trs { t, r, s } => (t, r, s),
                // an animated node with a matrix: the matrix is set aside (glTF forbids the pair)
                Local::Matrix(_) => ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
            };
            let v = c.sample(time);
            match c.path {
                Path::Translation => t = [v[0], v[1], v[2]],
                Path::Rotation => r = v,
                Path::Scale => s = [v[0], v[1], v[2]],
            }
            *l = Local::Trs { t, r, s };
        }
    }
    let mut out: Vec<Option<Mat4>> = vec![None; nodes.len()];
    fn resolve(k: usize, nodes: &[Node], local: &[Local], out: &mut Vec<Option<Mat4>>, depth: u32) -> Mat4 {
        if let Some(m) = out[k] {
            return m;
        }
        let parent = match nodes[k].parent {
            Some(p) if depth < 256 => resolve(p, nodes, local, out, depth + 1),
            _ => IDENTITY,
        };
        let m = mul(&parent, &local[k].matrix());
        out[k] = Some(m);
        m
    }
    (0..nodes.len()).map(|k| resolve(k, nodes, &local, &mut out, 0)).collect()
}

impl Channel {
    /// Its value at `time`: held before the first key and after the last.
    pub fn sample(&self, time: f32) -> [f32; 4] {
        let n = self.times.len().min(self.values.len());
        if n == 0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let next = self.times[..n].partition_point(|&k| k <= time);
        if next == 0 {
            return self.values[0];
        }
        if next >= n || self.step {
            return self.values[next - 1];
        }
        let (a, b) = (self.values[next - 1], self.values[next]);
        let span = self.times[next] - self.times[next - 1];
        let f = if span > 0.0 { (time - self.times[next - 1]) / span } else { 0.0 };
        if self.path == Path::Rotation {
            // the short way round, then back to unit length (a normalized lerp: close to a slerp between keys a tick apart)
            let d = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
            let b = if d < 0.0 { b.map(|x| -x) } else { b };
            let q: [f32; 4] = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f);
            let l = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            q.map(|x| x / l)
        } else {
            std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f)
        }
    }
}

pub(crate) fn trs_matrix(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = r;
    #[rustfmt::skip]
    let m = [
        (1.0 - 2.0 * (y * y + z * z)) * s[0], 2.0 * (x * y + z * w) * s[0], 2.0 * (x * z - y * w) * s[0], 0.0,
        2.0 * (x * y - z * w) * s[1], (1.0 - 2.0 * (x * x + z * z)) * s[1], 2.0 * (y * z + x * w) * s[1], 0.0,
        2.0 * (x * z + y * w) * s[2], 2.0 * (y * z - x * w) * s[2], (1.0 - 2.0 * (x * x + y * y)) * s[2], 0.0,
        t[0], t[1], t[2], 1.0,
    ];
    m
}

/// A node's rest transform as the file gives it.
pub(crate) fn local_of(node: &Value) -> Local {
    let floats = |key: &str, n: usize| -> Option<Vec<f32>> {
        let a = node.get(key)?.as_array()?;
        (a.len() == n).then(|| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
    };
    if let Some(m) = floats("matrix", 16) {
        return Local::Matrix(m.try_into().unwrap_or(IDENTITY));
    }
    let t = floats("translation", 3).unwrap_or_else(|| vec![0.0; 3]);
    let r = floats("rotation", 4).unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
    let s = floats("scale", 3).unwrap_or_else(|| vec![1.0; 3]);
    Local::Trs { t: [t[0], t[1], t[2]], r: [r[0], r[1], r[2], r[3]], s: [s[0], s[1], s[2]] }
}

/// Every node of the file, in its order, with its parent.
pub(crate) fn nodes(root: &Value) -> Vec<Node> {
    let list = root.get("nodes").and_then(Value::as_array).map_or(&[][..], |v| v.as_slice());
    let mut parent = vec![None; list.len()];
    for (k, n) in list.iter().enumerate() {
        for c in n.get("children").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64) {
            if let Some(p) = parent.get_mut(c as usize) {
                *p = Some(k);
            }
        }
    }
    list.iter()
        .zip(parent)
        .enumerate()
        .map(|(k, (n, parent))| Node {
            name: n
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map_or_else(|| format!("Node {k}"), str::to_string),
            parent,
            rest: local_of(n),
        })
        .collect()
}

/// The file's animations; `floats(accessor, components)` reads an accessor.
pub(crate) fn animations(root: &Value, floats: impl Fn(usize, usize) -> Result<Vec<f32>>) -> Result<Vec<Animation>> {
    let mut out = Vec::new();
    for (k, a) in root.get("animations").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let samplers = a.get("samplers").and_then(Value::as_array).map_or(&[][..], |v| v.as_slice());
        let mut channels = Vec::new();
        for c in a.get("channels").and_then(Value::as_array).into_iter().flatten() {
            let Some(node) = c.pointer("/target/node").and_then(Value::as_u64) else { continue };
            let path = match c.pointer("/target/path").and_then(Value::as_str) {
                Some("translation") => Path::Translation,
                Some("rotation") => Path::Rotation,
                Some("scale") => Path::Scale,
                _ => continue, // morph target weights are not read
            };
            let s = c
                .get("sampler")
                .and_then(Value::as_u64)
                .and_then(|s| samplers.get(s as usize))
                .ok_or_else(|| anyhow!("a channel has no sampler"))?;
            let input = s.get("input").and_then(Value::as_u64).ok_or_else(|| anyhow!("a sampler has no input"))?;
            let output = s.get("output").and_then(Value::as_u64).ok_or_else(|| anyhow!("a sampler has no output"))?;
            let width = if path == Path::Rotation { 4 } else { 3 };
            let times = floats(input as usize, 1)?;
            let flat = floats(output as usize, width)?;
            let interpolation = s.get("interpolation").and_then(Value::as_str).unwrap_or("LINEAR");
            let mut values: Vec<[f32; 4]> = flat
                .chunks_exact(width)
                .map(|v| if width == 4 { [v[0], v[1], v[2], v[3]] } else { [v[0], v[1], v[2], 0.0] })
                .collect();
            if interpolation == "CUBICSPLINE" {
                // in-tangent, value, out-tangent for each key: the values, moved between in a line
                values = values.chunks_exact(3).map(|t| t[1]).collect();
            }
            if values.len() != times.len() {
                bail!("an animation's keys and values differ in number");
            }
            channels.push(Channel { node: node as usize, path, times, values, step: interpolation == "STEP" });
        }
        let duration = channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max);
        out.push(Animation {
            name: a
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map_or_else(|| format!("Animation {}", k + 1), str::to_string),
            channels,
            duration,
            extras: a.get("extras").cloned(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::transform_point;

    fn channel(path: Path, step: bool, values: Vec<[f32; 4]>) -> Channel {
        Channel { node: 0, path, times: (0..values.len()).map(|k| k as f32).collect(), values, step }
    }

    #[test]
    fn a_channel_holds_steps_and_moves_between_keys() {
        let c = channel(Path::Translation, false, vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]]);
        assert_eq!(c.sample(-1.0)[0], 0.0, "before the first key: its value");
        assert_eq!(c.sample(0.25)[0], 0.5);
        assert_eq!(c.sample(9.0)[0], 2.0, "after the last: its value");
        let s = Channel { step: true, ..c };
        assert_eq!(s.sample(0.99)[0], 0.0, "a step holds until the next key");
        assert_eq!(s.sample(1.0)[0], 2.0);
        // a rotation goes the short way and stays unit
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let r = channel(Path::Rotation, false, vec![[0.0, 0.0, 0.0, 1.0], [0.0, -half, 0.0, -half]]);
        let q = r.sample(0.5);
        assert!((q.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(q[1] > 0.0 && q[3] > 0.0, "the second key flipped to the near side: {q:?}");
    }

    #[test]
    fn a_child_follows_its_animated_parent() {
        let nodes = vec![
            Node {
                name: "hips".into(),
                parent: None,
                rest: Local::Trs { t: [0.0, 1.0, 0.0], r: [0.0, 0.0, 0.0, 1.0], s: [1.0; 3] },
            },
            Node {
                name: "arm".into(),
                parent: Some(0),
                rest: Local::Trs { t: [1.0, 0.0, 0.0], r: [0.0, 0.0, 0.0, 1.0], s: [1.0; 3] },
            },
        ];
        let rest = world(&nodes, None);
        assert_eq!(transform_point(&rest[1], [0.0; 3]), [1.0, 1.0, 0.0]);
        // the hips moved 2 m along z by the animation: the arm comes along
        let a = Animation {
            name: "walk".into(),
            channels: vec![channel(Path::Translation, true, vec![[0.0, 1.0, 2.0, 0.0]])],
            duration: 0.0,
            extras: None,
        };
        let now = world(&nodes, Some((&a, 0.0)));
        assert_eq!(transform_point(&now[1], [0.0; 3]), [1.0, 1.0, 2.0]);
    }
}

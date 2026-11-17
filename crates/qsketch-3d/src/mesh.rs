//! Reads the geometry out of a binary glTF (`.glb`) file. Textures in the file are ignored:
//! the Model panel shows a qsketch document in their place. This is a small reader rather
//! than the `gltf` crate because that crate rejects textures which exist only through an
//! extension such as `EXT_texture_webp`, and generated models commonly use those.

use anyhow::{anyhow, bail, Context, Result};
use bytemuck::{Pod, Zeroable};
use serde_json::Value;

use crate::camera::{mul, transform_point, Mat4, IDENTITY};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// The file's named pieces, one per node with a mesh; empty when it has fewer than two, and in
    /// meshes made by qsketch's own tools.
    pub parts: Vec<Part>,
}

/// A named piece of a model, one glTF node's triangles, which the Model panel can show or hide.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    /// Parts in the same slot replace one another, as a character's helmets or the weapons on one arm
    /// do, so the panel shows one at a time. From the node's `extras.slot`.
    pub slot: Option<String>,
    /// Shown when the model opens: false when the node carries `KHR_node_visibility` with `visible: false`.
    pub visible: bool,
    pub triangles: std::ops::Range<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Like [`Mesh::bounds`], over the triangles not flagged in `hidden`: frames the parts on show rather
    /// than every variant a model carries. Falls back to the whole mesh when nothing would be left.
    pub fn bounds_without(&self, hidden: &[bool]) -> ([f32; 3], f32) {
        let mut shown = Mesh::default();
        for (t, tri) in self.indices.chunks_exact(3).enumerate() {
            if !hidden.get(t).copied().unwrap_or(false) {
                shown.vertices.extend(tri.iter().map(|&i| self.vertices[i as usize]));
            }
        }
        if shown.vertices.is_empty() {
            return self.bounds();
        }
        shown.bounds()
    }

    /// Center of the bounding box, and the radius of the sphere around it that holds the mesh.
    pub fn bounds(&self) -> ([f32; 3], f32) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &self.vertices {
            for a in 0..3 {
                min[a] = min[a].min(v.position[a]);
                max[a] = max[a].max(v.position[a]);
            }
        }
        if self.vertices.is_empty() {
            return ([0.0; 3], 1.0);
        }
        let center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
        let radius = self.vertices.iter().map(|v| length(sub(v.position, center))).fold(0.0, f32::max);
        (center, radius.max(1e-6))
    }

    /// Writes a binary glTF with `png` embedded as its color texture.
    pub fn to_glb(&self, png: &[u8]) -> Vec<u8> {
        fn align(bin: &mut Vec<u8>) {
            while bin.len() % 4 != 0 {
                bin.push(0);
            }
        }
        let mut bin = Vec::new();
        let mut views = Vec::new();
        let mut view = |bin: &mut Vec<u8>, data: &[u8]| {
            align(bin);
            views.push(serde_json::json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": data.len()}));
            bin.extend_from_slice(data);
            views.len() - 1
        };
        let attribute = |f: fn(&Vertex) -> &[f32]| -> Vec<u8> {
            self.vertices.iter().flat_map(|v| f(v).iter().flat_map(|c| c.to_le_bytes())).collect()
        };
        let positions = view(&mut bin, &attribute(|v| &v.position));
        let normals = view(&mut bin, &attribute(|v| &v.normal));
        let uvs = view(&mut bin, &attribute(|v| &v.uv));
        let indices = view(&mut bin, bytemuck::cast_slice(&self.indices));
        let image = view(&mut bin, png);
        align(&mut bin);

        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &self.vertices {
            for a in 0..3 {
                min[a] = min[a].min(v.position[a]);
                max[a] = max[a].max(v.position[a]);
            }
        }
        let count = self.vertices.len();
        let root = serde_json::json!({
            "asset": {"version": "2.0", "generator": "qsketch"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
                "indices": 3,
                "material": 0
            }]}],
            "materials": [{
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0.0, "roughnessFactor": 1.0},
                "doubleSided": true
            }],
            "textures": [{"source": 0}],
            "images": [{"bufferView": image, "mimeType": "image/png"}],
            "accessors": [
                {"bufferView": positions, "componentType": 5126, "count": count, "type": "VEC3", "min": min, "max": max},
                {"bufferView": normals, "componentType": 5126, "count": count, "type": "VEC3"},
                {"bufferView": uvs, "componentType": 5126, "count": count, "type": "VEC2"},
                {"bufferView": indices, "componentType": 5125, "count": self.indices.len(), "type": "SCALAR"}
            ],
            "bufferViews": views,
            "buffers": [{"byteLength": bin.len()}]
        });
        let mut json = serde_json::to_vec(&root).unwrap_or_default();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut out = Vec::with_capacity(28 + json.len() + bin.len());
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((28 + json.len() + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E4942u32.to_le_bytes());
        out.extend_from_slice(&bin);
        out
    }

    pub fn from_glb(bytes: &[u8]) -> Result<Self> {
        let (json, bin) = split_glb(bytes)?;
        let root: Value = serde_json::from_slice(json).context("the file's JSON is invalid")?;
        let doc = Doc { root: &root, bin };
        if let Some(required) = root.get("extensionsRequired").and_then(Value::as_array) {
            for name in ["KHR_draco_mesh_compression", "EXT_meshopt_compression"] {
                if required.iter().any(|e| e.as_str() == Some(name)) {
                    bail!("the model is compressed ({name}), which is not supported; export it without compression");
                }
            }
        }

        let mut mesh = Mesh::default();
        let scene = root.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
        let roots: Vec<usize> = match root.pointer(&format!("/scenes/{scene}/nodes")).and_then(Value::as_array) {
            Some(nodes) => nodes.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect(),
            // No scene: every node nobody names as a child is a root.
            None => doc.parentless_nodes(),
        };
        for node in roots {
            doc.add_node(node, &IDENTITY, 0, &mut mesh)?;
        }
        if mesh.indices.is_empty() {
            bail!("the file has no triangles");
        }
        if mesh.parts.len() < 2 {
            mesh.parts.clear();
        }
        Ok(mesh)
    }
}

/// An image stored inside a model file, still encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedImage {
    pub bytes: Vec<u8>,
    /// The file extension for its format: `png`, `jpg` or `webp`.
    pub extension: &'static str,
}

/// The base color image's index: the first material's, or failing that the first image in the file.
fn base_color_image(root: &Value) -> usize {
    let index = |v: &Value, path: &str| v.pointer(path).and_then(Value::as_u64);
    let from_material = || {
        let material = index(root, "/meshes/0/primitives/0/material").unwrap_or(0);
        let texture = index(root, &format!("/materials/{material}/pbrMetallicRoughness/baseColorTexture/index"))?;
        let texture = root.pointer(&format!("/textures/{texture}"))?;
        index(texture, "/source").or_else(|| index(texture, "/extensions/EXT_texture_webp/source"))
    };
    from_material().unwrap_or(0) as usize
}

/// A rectangle of the texture whose pixels may take only its own colors (a console texture page and its color
/// table); a color with alpha 0 is see-through.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteRegion {
    pub rect: [u32; 4],
    pub colors: Vec<[u8; 4]>,
    /// What the region is, for the panel (`"name"`, optional).
    pub name: Option<String>,
}

/// The palette regions of the base color image, from its `extras.palette_regions`: `{"size": [w, h], "regions":
/// [{"rect": [x, y, w, h], "colors": [[r, g, b, a], ...], "name": "..."}]}` (the name optional), and the image
/// size they were written for. Empty
/// when the file has none or they are malformed.
pub fn palette_regions(glb: &[u8]) -> Option<([u32; 2], Vec<PaletteRegion>)> {
    let (json, _) = split_glb(glb).ok()?;
    let root: Value = serde_json::from_slice(json).ok()?;
    let extras = root.pointer(&format!("/images/{}/extras/palette_regions", base_color_image(&root)))?;
    let numbers = |v: &Value| -> Option<Vec<u32>> {
        v.as_array()?.iter().map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok())).collect()
    };
    let size = numbers(extras.get("size")?)?;
    let regions = extras
        .get("regions")?
        .as_array()?
        .iter()
        .map(|r| {
            let rect = numbers(r.get("rect")?)?;
            let colors = r
                .get("colors")?
                .as_array()?
                .iter()
                .map(|c| {
                    let c = numbers(c)?;
                    (c.len() == 4 && c.iter().all(|&v| v < 256))
                        .then(|| [c[0] as u8, c[1] as u8, c[2] as u8, c[3] as u8])
                })
                .collect::<Option<Vec<_>>>()?;
            let name = r.get("name").and_then(Value::as_str).map(str::to_string);
            (rect.len() == 4 && !colors.is_empty()).then(|| PaletteRegion {
                rect: [rect[0], rect[1], rect[2], rect[3]],
                colors,
                name,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (size.len() == 2 && !regions.is_empty()).then(|| ([size[0], size[1]], regions))
}

/// The color texture stored inside a `.glb`, if it has one: the first material's base color image, or failing
/// that the first image in the file.
pub fn embedded_texture(glb: &[u8]) -> Option<EmbeddedImage> {
    let (json, bin) = split_glb(glb).ok()?;
    let root: Value = serde_json::from_slice(json).ok()?;
    let index = |v: &Value, path: &str| v.pointer(path).and_then(Value::as_u64);
    let image = root.pointer(&format!("/images/{}", base_color_image(&root)))?;
    let view = root.pointer(&format!("/bufferViews/{}", index(image, "/bufferView")?))?;
    if index(view, "/buffer").unwrap_or(0) != 0 {
        return None;
    }
    let start = index(view, "/byteOffset").unwrap_or(0) as usize;
    let bytes = bin.get(start..start.checked_add(index(view, "/byteLength")? as usize)?)?;
    // The bytes say what they are more reliably than the file's own label.
    let extension = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        _ => return None,
    };
    Some(EmbeddedImage { bytes: bytes.to_vec(), extension })
}

/// How a model asks for its texture to be drawn, from its first primitive's material.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextureLook {
    /// The base color texture's sampler magnifies with `NEAREST`: crisp texels, as pixel art and console
    /// models such as the PlayStation's are drawn.
    pub nearest: bool,
    /// The material's `alphaMode` is `MASK`: texels with less alpha than this are not drawn at all.
    pub alpha_cutoff: Option<f32>,
}

/// Reads the `TextureLook` of a `.glb`; a file that says nothing gets the defaults (smooth, opaque).
pub fn texture_look(glb: &[u8]) -> TextureLook {
    let Some(root) = split_glb(glb).ok().and_then(|(json, _)| serde_json::from_slice::<Value>(json).ok()) else {
        return TextureLook::default();
    };
    let material = root.pointer("/meshes/0/primitives/0/material").and_then(Value::as_u64).unwrap_or(0);
    let Some(material) = root.pointer(&format!("/materials/{material}")) else { return TextureLook::default() };
    let sampler = material
        .pointer("/pbrMetallicRoughness/baseColorTexture/index")
        .and_then(Value::as_u64)
        .and_then(|t| root.pointer(&format!("/textures/{t}/sampler")))
        .and_then(Value::as_u64)
        .and_then(|s| root.pointer(&format!("/samplers/{s}")));
    const NEAREST: u64 = 9728;
    let nearest = sampler.and_then(|s| s.get("magFilter")).and_then(Value::as_u64) == Some(NEAREST);
    let alpha_cutoff = (material.get("alphaMode").and_then(Value::as_str) == Some("MASK"))
        .then(|| material.get("alphaCutoff").and_then(Value::as_f64).unwrap_or(0.5) as f32);
    TextureLook { nearest, alpha_cutoff }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalized(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn split_glb(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let u32_at = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?)) };
    if bytes.get(..4) != Some(b"glTF") {
        bail!("not a binary glTF (.glb) file");
    }
    if u32_at(4) != Some(2) {
        bail!("only glTF 2 is supported");
    }
    let (mut json, mut bin) = (None, &[][..]);
    let mut at = 12usize;
    while let (Some(len), Some(kind)) = (u32_at(at), u32_at(at + 4)) {
        let end = (at + 8)
            .checked_add(len as usize)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| anyhow!("the file is cut short"))?;
        let chunk = &bytes[at + 8..end];
        match kind {
            0x4E4F534A => json = Some(chunk), // "JSON"
            0x004E4942 => bin = chunk,        // "BIN\0"
            _ => {}
        }
        at = end;
    }
    Ok((json.ok_or_else(|| anyhow!("the file has no JSON chunk"))?, bin))
}

struct Doc<'a> {
    root: &'a Value,
    bin: &'a [u8],
}

const MAX_NODE_DEPTH: u32 = 256; // a node cycle in a damaged file would otherwise never end

impl Doc<'_> {
    fn parentless_nodes(&self) -> Vec<usize> {
        let nodes = self.root.get("nodes").and_then(Value::as_array).map_or(&[][..], |v| v.as_slice());
        let children: std::collections::HashSet<u64> =
            nodes.iter().filter_map(|n| n.get("children")?.as_array()).flatten().filter_map(Value::as_u64).collect();
        (0..nodes.len()).filter(|&i| !children.contains(&(i as u64))).collect()
    }

    fn add_node(&self, index: usize, parent: &Mat4, depth: u32, out: &mut Mesh) -> Result<()> {
        if depth > MAX_NODE_DEPTH {
            bail!("the node hierarchy is too deep");
        }
        let node = self.root.pointer(&format!("/nodes/{index}")).ok_or_else(|| anyhow!("missing node {index}"))?;
        let world = mul(parent, &node_matrix(node));
        if let Some(mesh) = node.get("mesh").and_then(Value::as_u64) {
            let path = format!("/meshes/{mesh}/primitives");
            let prims =
                self.root.pointer(&path).and_then(Value::as_array).ok_or_else(|| anyhow!("missing mesh {mesh}"))?;
            let first = out.triangle_count() as u32;
            for prim in prims {
                self.add_primitive(prim, &world, out)?;
            }
            let end = out.triangle_count() as u32;
            if end > first {
                let text = |v: Option<&Value>| v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
                let name = text(node.get("name"))
                    .or_else(|| text(self.root.pointer(&format!("/meshes/{mesh}/name"))))
                    .unwrap_or_else(|| format!("Part {}", out.parts.len() + 1));
                let visible = node.pointer("/extensions/KHR_node_visibility/visible").and_then(Value::as_bool);
                out.parts.push(Part {
                    name,
                    slot: text(node.pointer("/extras/slot")),
                    visible: visible.unwrap_or(true),
                    triangles: first..end,
                });
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children.iter().filter_map(Value::as_u64) {
                self.add_node(child as usize, &world, depth + 1, out)?;
            }
        }
        Ok(())
    }

    fn add_primitive(&self, prim: &Value, world: &Mat4, out: &mut Mesh) -> Result<()> {
        if prim.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
            return Ok(()); // lines, points and strips are not paintable surfaces
        }
        let attr = |name: &str| prim.pointer(&format!("/attributes/{name}")).and_then(Value::as_u64);
        let position = attr("POSITION").ok_or_else(|| anyhow!("a primitive has no POSITION"))?;
        let positions = self.floats(position as usize, 3)?;
        let count = positions.len() / 3;

        let uvs = match attr("TEXCOORD_0") {
            Some(a) => self.floats(a as usize, 2)?,
            None => vec![0.0; count * 2],
        };
        let normals = attr("NORMAL").map(|a| self.floats(a as usize, 3)).transpose()?;
        if uvs.len() != count * 2 || normals.as_ref().is_some_and(|n| n.len() != count * 3) {
            bail!("a primitive's attributes have different lengths");
        }
        let indices = match prim.get("indices").and_then(Value::as_u64) {
            Some(a) => self.indices(a as usize)?,
            None => (0..count as u32).collect(),
        };
        if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= count) {
            bail!("a primitive has invalid indices");
        }

        // Normals take the cofactor matrix, which stays correct under non-uniform scale.
        let (x, y, z) =
            ([world[0], world[1], world[2]], [world[4], world[5], world[6]], [world[8], world[9], world[10]]);
        let (cx, cy, cz) = (cross(y, z), cross(z, x), cross(x, y));
        let mirrored = dot(x, cx) < 0.0;
        let sign = if mirrored { -1.0 } else { 1.0 };
        let normal_of = |n: [f32; 3]| {
            normalized([
                sign * (cx[0] * n[0] + cy[0] * n[1] + cz[0] * n[2]),
                sign * (cx[1] * n[0] + cy[1] * n[1] + cz[1] * n[2]),
                sign * (cx[2] * n[0] + cy[2] * n[1] + cz[2] * n[2]),
            ])
        };

        let base = u32::try_from(out.vertices.len()).context("the model is too large")?;
        for i in 0..count {
            let p = transform_point(world, [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]]);
            let normal = normals.as_ref().map_or([0.0; 3], |n| normal_of([n[i * 3], n[i * 3 + 1], n[i * 3 + 2]]));
            out.vertices.push(Vertex { position: p, normal, uv: [uvs[i * 2], uvs[i * 2 + 1]] });
        }
        let first_index = out.indices.len();
        for tri in indices.chunks_exact(3) {
            let order = if mirrored { [tri[0], tri[2], tri[1]] } else { [tri[0], tri[1], tri[2]] };
            out.indices.extend(order.iter().map(|i| base + i));
        }
        if normals.is_none() {
            smooth_normals(&mut out.vertices, &out.indices[first_index..]);
        }
        Ok(())
    }

    /// The accessor's raw bytes per element, element count, component type and component count.
    fn accessor(&self, index: usize, want: Option<usize>) -> Result<Accessor<'_>> {
        let a = self.root.pointer(&format!("/accessors/{index}")).ok_or_else(|| anyhow!("missing accessor {index}"))?;
        if a.get("sparse").is_some() {
            bail!("sparse accessors are not supported");
        }
        let view = a.get("bufferView").and_then(Value::as_u64).ok_or_else(|| anyhow!("an accessor has no buffer"))?;
        let v =
            self.root.pointer(&format!("/bufferViews/{view}")).ok_or_else(|| anyhow!("missing buffer view {view}"))?;
        if v.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
            bail!("only the file's embedded buffer is supported");
        }
        let int = |o: &Value, k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        let components = match a.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            other => bail!("unsupported accessor type {other:?}"),
        };
        if want.is_some_and(|w| w != components) {
            bail!("an accessor has the wrong shape");
        }
        let kind = int(a, "componentType");
        let size = match kind {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => bail!("unsupported component type {kind}"),
        };
        let count = int(a, "count");
        let element = components * size;
        let stride = match int(v, "byteStride") {
            0 => element,
            s => s,
        };
        if stride < element {
            bail!("a buffer view's stride is too small");
        }
        let start = int(v, "byteOffset").checked_add(int(a, "byteOffset")).ok_or_else(|| anyhow!("bad offset"))?;
        let end = match count {
            0 => start,
            n => (n - 1)
                .checked_mul(stride)
                .and_then(|b| b.checked_add(start + element))
                .ok_or_else(|| anyhow!("bad size"))?,
        };
        let data = self.bin.get(start..end).ok_or_else(|| anyhow!("an accessor runs past the end of the file"))?;
        let normalized = a.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        Ok(Accessor { data, stride, count, components, size, kind, normalized })
    }

    fn floats(&self, index: usize, components: usize) -> Result<Vec<f32>> {
        let a = self.accessor(index, Some(components))?;
        let mut out = Vec::with_capacity(a.count * a.components);
        for e in 0..a.count {
            for c in 0..a.components {
                out.push(a.float(e, c)?);
            }
        }
        Ok(out)
    }

    fn indices(&self, index: usize) -> Result<Vec<u32>> {
        let a = self.accessor(index, Some(1))?;
        (0..a.count).map(|e| a.int(e)).collect()
    }
}

struct Accessor<'a> {
    data: &'a [u8],
    stride: usize,
    count: usize,
    components: usize,
    size: usize,
    kind: usize,
    normalized: bool,
}

impl Accessor<'_> {
    fn bytes(&self, element: usize, component: usize) -> [u8; 4] {
        let at = element * self.stride + component * self.size;
        let mut out = [0; 4];
        out[..self.size].copy_from_slice(&self.data[at..at + self.size]);
        out
    }

    fn float(&self, element: usize, component: usize) -> Result<f32> {
        let raw = self.bytes(element, component);
        Ok(match (self.kind, self.normalized) {
            (5126, _) => f32::from_le_bytes(raw),
            (5121, true) => f32::from(raw[0]) / 255.0,
            (5123, true) => f32::from(u16::from_le_bytes([raw[0], raw[1]])) / 65535.0,
            (5120, true) => (f32::from(raw[0] as i8) / 127.0).max(-1.0),
            (5122, true) => (f32::from(i16::from_le_bytes([raw[0], raw[1]])) / 32767.0).max(-1.0),
            _ => bail!("attribute data must be float or normalized"),
        })
    }

    fn int(&self, element: usize) -> Result<u32> {
        let raw = self.bytes(element, 0);
        match self.kind {
            5121 | 5123 | 5125 => Ok(u32::from_le_bytes(raw)),
            _ => bail!("indices must be unsigned integers"),
        }
    }
}

fn smooth_normals(vertices: &mut [Vertex], indices: &[u32]) {
    let touched: Vec<usize> = indices.iter().map(|&i| i as usize).collect();
    for &i in &touched {
        vertices[i].normal = [0.0; 3];
    }
    for tri in touched.chunks_exact(3) {
        let (a, b, c) = (vertices[tri[0]].position, vertices[tri[1]].position, vertices[tri[2]].position);
        let face = cross(sub(b, a), sub(c, a));
        for &i in tri {
            let n = &mut vertices[i].normal;
            *n = [n[0] + face[0], n[1] + face[1], n[2] + face[2]];
        }
    }
    for &i in &touched {
        vertices[i].normal = normalized(vertices[i].normal);
    }
}

/// A node's local transform, from `matrix` or from its translation, rotation and scale.
fn node_matrix(node: &Value) -> Mat4 {
    let floats = |key: &str, n: usize| -> Option<Vec<f32>> {
        let a = node.get(key)?.as_array()?;
        (a.len() == n).then(|| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
    };
    if let Some(m) = floats("matrix", 16) {
        return m.try_into().unwrap_or(IDENTITY);
    }
    let t = floats("translation", 3).unwrap_or_else(|| vec![0.0; 3]);
    let r = floats("rotation", 4).unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
    let s = floats("scale", 3).unwrap_or_else(|| vec![1.0; 3]);
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    #[rustfmt::skip]
    let m = [
        (1.0 - 2.0 * (y * y + z * z)) * s[0], 2.0 * (x * y + z * w) * s[0], 2.0 * (x * z - y * w) * s[0], 0.0,
        2.0 * (x * y - z * w) * s[1], (1.0 - 2.0 * (x * x + z * z)) * s[1], 2.0 * (y * z + x * w) * s[1], 0.0,
        2.0 * (x * z + y * w) * s[2], 2.0 * (y * z - x * w) * s[2], (1.0 - 2.0 * (x * x + y * y)) * s[2], 0.0,
        t[0], t[1], t[2], 1.0,
    ];
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One triangle in the XY plane, facing +Z, with u16 indices.
    fn triangle_bin() -> Vec<u8> {
        let mut bin = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend(f.to_le_bytes());
        }
        for f in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
            bin.extend(f.to_le_bytes());
        }
        for i in [0u16, 1, 2] {
            bin.extend(i.to_le_bytes());
        }
        bin
    }

    fn triangle_json(node: Value) -> Value {
        json!({
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [node],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2}]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24},
                {"buffer": 0, "byteOffset": 60, "byteLength": 6}
            ],
            // Like generated models: an image known only through an extension.
            "textures": [{"extensions": {"EXT_texture_webp": {"source": 0}}}]
        })
    }

    fn glb(json: &Value, bin: &[u8]) -> Vec<u8> {
        let mut json = serde_json::to_vec(json).unwrap();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(0x4E4F534Au32.to_le_bytes());
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004E4942u32.to_le_bytes());
        out.extend(&bin);
        out
    }

    #[test]
    fn reads_a_triangle_and_ignores_extension_textures() {
        let m = Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0})), &triangle_bin())).unwrap();
        assert_eq!(m.triangle_count(), 1);
        assert_eq!(m.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(m.vertices[1].uv, [1.0, 0.0]);
        assert_eq!(m.vertices[0].normal, [0.0, 0.0, 1.0], "missing normals are computed");
    }

    #[test]
    fn reads_the_texture_look_from_the_material_and_its_sampler() {
        let mut json = triangle_json(json!({"mesh": 0}));
        assert_eq!(
            texture_look(&glb(&json, &triangle_bin())),
            TextureLook::default(),
            "no material: smooth and opaque"
        );
        json["meshes"][0]["primitives"][0]["material"] = json!(0);
        json["materials"] = json!([{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}, "alphaMode": "MASK"}]);
        json["textures"] = json!([{"source": 0, "sampler": 0}]);
        json["samplers"] = json!([{"magFilter": 9728, "minFilter": 9728}]);
        let look = texture_look(&glb(&json, &triangle_bin()));
        assert_eq!(
            look,
            TextureLook { nearest: true, alpha_cutoff: Some(0.5) },
            "MASK without a cutoff uses glTF's 0.5"
        );
        json["samplers"][0]["magFilter"] = json!(9729);
        json["materials"][0]["alphaMode"] = json!("BLEND");
        assert_eq!(texture_look(&glb(&json, &triangle_bin())), TextureLook::default());
    }

    #[test]
    fn reads_named_nodes_as_parts_with_their_slots_and_visibility() {
        let mut json = triangle_json(json!({"mesh": 0, "name": "Helmet", "extras": {"slot": "Head"}}));
        json["nodes"].as_array_mut().unwrap().push(json!({
            "mesh": 0, "name": "Hair", "extras": {"slot": "Head"},
            "extensions": {"KHR_node_visibility": {"visible": false}}
        }));
        json["nodes"].as_array_mut().unwrap().push(json!({"mesh": 0}));
        json["scenes"][0]["nodes"] = json!([0, 1, 2]);
        let m = Mesh::from_glb(&glb(&json, &triangle_bin())).unwrap();
        let head = Some("Head".to_string());
        assert_eq!(m.parts.len(), 3);
        assert_eq!(m.parts[0], Part { name: "Helmet".into(), slot: head.clone(), visible: true, triangles: 0..1 });
        assert_eq!(m.parts[1], Part { name: "Hair".into(), slot: head, visible: false, triangles: 1..2 });
        assert_eq!((m.parts[2].name.as_str(), m.parts[2].slot.as_deref(), m.parts[2].visible), ("Part 3", None, true));
        let single = Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0, "name": "Only"})), &triangle_bin())).unwrap();
        assert!(single.parts.is_empty(), "one part is the whole model, not a choice");
    }

    #[test]
    fn reads_the_base_color_images_palette_regions() {
        let mut json = triangle_json(json!({"mesh": 0}));
        assert_eq!(palette_regions(&glb(&json, &triangle_bin())), None);
        json["images"] = json!([{"extras": {"palette_regions": {"size": [64, 32], "regions": [
            {"rect": [0, 0, 32, 32], "colors": [[0, 0, 0, 0], [255, 0, 0, 255]], "name": "left"},
            {"rect": [32, 0, 32, 32], "colors": [[0, 0, 255, 255]]}
        ]}}}]);
        let (size, regions) = palette_regions(&glb(&json, &triangle_bin())).unwrap();
        assert_eq!(size, [64, 32]);
        let left = PaletteRegion {
            rect: [0, 0, 32, 32],
            colors: vec![[0, 0, 0, 0], [255, 0, 0, 255]],
            name: Some("left".into()),
        };
        assert_eq!(regions[0], left);
        assert_eq!((regions[1].colors.clone(), regions[1].name.clone()), (vec![[0, 0, 255, 255]], None));
        json["images"][0]["extras"]["palette_regions"]["regions"][1]["colors"] = json!([[0, 0, 300, 255]]);
        assert_eq!(palette_regions(&glb(&json, &triangle_bin())), None, "a color out of range is refused, not wrapped");
    }

    #[test]
    fn applies_node_translation() {
        let m =
            Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0, "translation": [5.0, 0.0, 0.0]})), &triangle_bin()));
        assert_eq!(m.unwrap().vertices[0].position, [5.0, 0.0, 0.0]);
    }

    #[test]
    fn a_mirroring_transform_keeps_triangles_facing_out() {
        let node = json!({"mesh": 0, "scale": [-1.0, 1.0, 1.0]});
        let m = Mesh::from_glb(&glb(&triangle_json(node), &triangle_bin())).unwrap();
        let p = |i: usize| m.vertices[m.indices[i] as usize].position;
        let face = cross(sub(p(1), p(0)), sub(p(2), p(0)));
        assert!(dot(face, m.vertices[0].normal) > 0.0);
    }

    #[test]
    fn rejects_bad_files_without_panicking() {
        assert!(Mesh::from_glb(b"nope").is_err());
        let good = glb(&triangle_json(json!({"mesh": 0})), &triangle_bin());
        assert!(Mesh::from_glb(&good[..good.len() - 30]).is_err());

        let mut bad_index = triangle_bin();
        bad_index[60] = 9;
        assert!(Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0})), &bad_index)).is_err());

        let mut json = triangle_json(json!({"mesh": 0}));
        json["accessors"][0]["count"] = json!(4_000_000_000u64);
        assert!(Mesh::from_glb(&glb(&json, &triangle_bin())).is_err());
    }

    #[test]
    fn a_node_cycle_is_an_error() {
        let mut json = triangle_json(json!({"mesh": 0, "children": [0]}));
        json["scenes"][0]["nodes"] = json!([0]);
        assert!(Mesh::from_glb(&glb(&json, &triangle_bin())).is_err());
    }

    #[test]
    fn a_written_glb_reads_back_the_same() {
        let original = Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0})), &triangle_bin())).unwrap();
        let back = Mesh::from_glb(&original.to_glb(b"not really a png")).unwrap();
        assert_eq!(back.vertices, original.vertices);
        assert_eq!(back.indices, original.indices);
    }

    #[test]
    fn the_texture_inside_a_model_can_be_read_back() {
        let mesh = Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0})), &triangle_bin())).unwrap();
        let png = b"\x89PNG\r\n\x1a\n and the rest";
        let found = embedded_texture(&mesh.to_glb(png)).unwrap();
        assert_eq!((found.bytes.as_slice(), found.extension), (&png[..], "png"));
        assert_eq!(embedded_texture(&glb(&triangle_json(json!({"mesh": 0})), &triangle_bin())), None);
        assert_eq!(embedded_texture(&mesh.to_glb(b"not an image")), None);
    }

    #[test]
    fn a_compressed_model_says_so() {
        let mut json = triangle_json(json!({"mesh": 0}));
        json["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
        let error = Mesh::from_glb(&glb(&json, &triangle_bin())).unwrap_err();
        assert!(format!("{error}").contains("compressed"), "{error}");
    }

    #[test]
    fn bounds_cover_the_mesh() {
        let m = Mesh::from_glb(&glb(&triangle_json(json!({"mesh": 0})), &triangle_bin())).unwrap();
        let (center, radius) = m.bounds();
        assert_eq!(center, [0.5, 0.5, 0.0]);
        assert!((radius - 0.5f32.sqrt()).abs() < 1e-6);
    }
}

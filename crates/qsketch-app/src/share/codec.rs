//! The payloads a shared canvas exchanges over Leyline: a whole-document
//! snapshot, a tile patch of one layer, and the layer stack. Leyline carries
//! them as opaque blobs; both ends of a session run this code.
//!
//! Container: `QSKL` · version byte · little-endian u32 header length · JSON
//! header · payload. Pixels travel as PNG (fast compression) — a patch's PNG
//! covers the tile-aligned bounding box of the changed tiles, and the header's
//! `mask` says which tiles inside that box the receiver should actually take,
//! so an L-shaped stroke does not overwrite the untouched corner of its box.

use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use qsketch_core::{DocState, Layer, LayerId, LayerKind, Raster, Tile, TILE};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 4] = b"QSKL";
const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
pub struct SnapshotHeader {
    pub w: u32,
    pub h: u32,
    pub title: String,
    pub active: LayerId,
    pub layers: Vec<qsketch_core::layer::LayerProps>,
    /// PNG byte length per layer, in `layers` order (0 for a group).
    pub pngs: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct PatchHeader {
    pub w: u32,
    pub h: u32,
    pub layer: LayerId,
    /// Tile-aligned box: first tile column/row and size in tiles.
    pub tx: u32,
    pub ty: u32,
    pub tw: u32,
    pub th: u32,
    /// Row-major over the box: which tiles the receiver takes.
    pub mask: Vec<bool>,
}

#[derive(Serialize, Deserialize)]
pub struct LayersHeader {
    pub w: u32,
    pub h: u32,
    pub active: LayerId,
    pub layers: Vec<qsketch_core::layer::LayerProps>,
}

fn pack<H: Serialize>(header: &H, payload: &[u8]) -> Vec<u8> {
    let hdr = serde_json::to_vec(header).unwrap_or_default();
    let mut out = Vec::with_capacity(9 + hdr.len() + payload.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(hdr.len() as u32).to_le_bytes());
    out.extend_from_slice(&hdr);
    out.extend_from_slice(payload);
    out
}

fn unpack<H: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<(H, &[u8])> {
    if bytes.len() < 9 || &bytes[..4] != MAGIC {
        bail!("not a shared-canvas payload");
    }
    if bytes[4] != VERSION {
        bail!("shared-canvas payload version {} (this build speaks {VERSION})", bytes[4]);
    }
    let n = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
    let hdr = bytes.get(9..9 + n).ok_or_else(|| anyhow!("truncated header"))?;
    Ok((serde_json::from_slice(hdr)?, &bytes[9 + n..]))
}

fn png_encode(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut out = Cursor::new(Vec::new());
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub).write_image(
        rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out.into_inner())
}

fn png_decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.into_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

// --- snapshot -------------------------------------------------------------

pub fn encode_snapshot(state: &DocState, title: &str) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    let mut pngs = Vec::with_capacity(state.layers.len());
    for l in &state.layers {
        if l.is_group() {
            pngs.push(0);
            continue;
        }
        let png = png_encode(state.width, state.height, &l.raster.to_rgba())?;
        pngs.push(png.len() as u32);
        payload.extend_from_slice(&png);
    }
    let header = SnapshotHeader {
        w: state.width,
        h: state.height,
        title: title.to_string(),
        active: state.active_layer().props.id,
        layers: state.layers.iter().map(|l| l.props.clone()).collect(),
        pngs,
    };
    Ok(pack(&header, &payload))
}

/// A document state rebuilt from a snapshot, plus its title.
pub fn decode_snapshot(bytes: &[u8]) -> Result<(DocState, String)> {
    let (h, mut payload): (SnapshotHeader, &[u8]) = unpack(bytes)?;
    if h.w == 0 || h.h == 0 || h.w > 32768 || h.h > 32768 || h.pngs.len() != h.layers.len() {
        bail!("malformed snapshot");
    }
    let mut layers = Vec::with_capacity(h.layers.len());
    for (props, &len) in h.layers.iter().zip(&h.pngs) {
        let raster = if props.kind == LayerKind::Group || len == 0 {
            Raster::new(h.w, h.h)
        } else {
            let png = payload.get(..len as usize).ok_or_else(|| anyhow!("truncated snapshot"))?;
            payload = &payload[len as usize..];
            let (pw, ph, rgba) = png_decode(png)?;
            if (pw, ph) != (h.w, h.h) {
                bail!("layer image is {pw}×{ph}, document is {}×{}", h.w, h.h);
            }
            Raster::from_rgba(h.w, h.h, &rgba)
        };
        layers.push(Layer { props: props.clone(), raster });
    }
    if layers.is_empty() {
        bail!("snapshot has no layers");
    }
    let active = layers.iter().position(|l| l.props.id == h.active).unwrap_or(0);
    let next = layers.iter().map(|l| l.props.id).max().unwrap_or(0) + 1;
    Ok((DocState { width: h.w, height: h.h, layers, active, selection: None, next_layer_id: next }, h.title))
}

// --- patch ----------------------------------------------------------------

/// Encode the given tiles (indices into the raster's tile grid) of one layer.
pub fn encode_patch(state: &DocState, layer: LayerId, tiles: &[usize]) -> Result<Vec<u8>> {
    let l = state.layer_by_id(layer).ok_or_else(|| anyhow!("no such layer"))?;
    let r = &l.raster;
    let (txn, _) = (r.tiles_x(), r.tiles_y());
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for &i in tiles {
        let (tx, ty) = (i as u32 % txn, i as u32 / txn);
        x0 = x0.min(tx);
        y0 = y0.min(ty);
        x1 = x1.max(tx);
        y1 = y1.max(ty);
    }
    if tiles.is_empty() {
        bail!("empty patch");
    }
    let (tw, th) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut mask = vec![false; (tw * th) as usize];
    for &i in tiles {
        let (tx, ty) = (i as u32 % txn, i as u32 / txn);
        mask[((ty - y0) * tw + (tx - x0)) as usize] = true;
    }
    // Pixel box, clipped to the document.
    let px = x0 * TILE as u32;
    let py = y0 * TILE as u32;
    let pw = (tw * TILE as u32).min(state.width - px);
    let ph = (th * TILE as u32).min(state.height - py);
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    let stride = pw as usize * 4;
    for ty in 0..th {
        for tx in 0..tw {
            if !mask[(ty * tw + tx) as usize] {
                continue;
            }
            let Some(t) = r.tile(x0 + tx, y0 + ty) else { continue };
            let ox = (tx as usize) * TILE;
            let oy = (ty as usize) * TILE;
            let w = TILE.min(pw as usize - ox);
            let h = TILE.min(ph as usize - oy);
            for ly in 0..h {
                rgba[(oy + ly) * stride + ox * 4..][..w * 4].copy_from_slice(&t.px[ly * TILE * 4..][..w * 4]);
            }
        }
    }
    let png = png_encode(pw, ph, &rgba)?;
    let header = PatchHeader { w: state.width, h: state.height, layer, tx: x0, ty: y0, tw, th, mask };
    Ok(pack(&header, &png))
}

/// A decoded patch: the layer, and each affected tile as `(index, tile)` where
/// `None` is a fully transparent tile.
pub struct Patch {
    pub w: u32,
    pub h: u32,
    pub layer: LayerId,
    pub tiles: Vec<(usize, Option<Arc<Tile>>)>,
}

pub fn decode_patch(bytes: &[u8]) -> Result<Patch> {
    let (h, png): (PatchHeader, &[u8]) = unpack(bytes)?;
    if h.mask.len() != (h.tw * h.th) as usize || h.w == 0 || h.h == 0 {
        bail!("malformed patch");
    }
    let (pw, ph, rgba) = png_decode(png)?;
    let px = h.tx * TILE as u32;
    let py = h.ty * TILE as u32;
    if px >= h.w || py >= h.h || pw != (h.tw * TILE as u32).min(h.w - px) || ph != (h.th * TILE as u32).min(h.h - py) {
        bail!("patch box does not match its image");
    }
    // The box is tile-aligned, so a raster of the box has exactly its tiles.
    let boxed = Raster::from_rgba(pw, ph, &rgba);
    let txn = h.w.div_ceil(TILE as u32);
    let mut tiles = Vec::new();
    for ty in 0..h.th {
        for tx in 0..h.tw {
            if !h.mask[(ty * h.tw + tx) as usize] {
                continue;
            }
            let idx = ((h.ty + ty) * txn + (h.tx + tx)) as usize;
            tiles.push((idx, boxed.tile(tx, ty).cloned()));
        }
    }
    Ok(Patch { w: h.w, h: h.h, layer: h.layer, tiles })
}

// --- layers ---------------------------------------------------------------

pub fn encode_layers(state: &DocState) -> Vec<u8> {
    let header = LayersHeader {
        w: state.width,
        h: state.height,
        active: state.active_layer().props.id,
        layers: state.layers.iter().map(|l| l.props.clone()).collect(),
    };
    pack(&header, &[])
}

pub fn decode_layers(bytes: &[u8]) -> Result<LayersHeader> {
    let (h, _): (LayersHeader, &[u8]) = unpack(bytes)?;
    if h.layers.is_empty() {
        bail!("layer stack is empty");
    }
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qsketch_core::Rgba8;

    #[test]
    fn snapshot_round_trips_layers_and_pixels() {
        let mut s = DocState::new(200, 130, None);
        s.layers[0].raster.set_pixel(5, 7, Rgba8::new(10, 20, 30, 255));
        let id = s.add_layer("Ink", None);
        s.layer_by_id(id).unwrap();
        s.layers[1].raster.set_pixel(150, 100, Rgba8::new(1, 2, 3, 4));
        s.layers[1].props.opacity = 0.5;
        let bytes = encode_snapshot(&s, "Doodle").unwrap();
        let (back, title) = decode_snapshot(&bytes).unwrap();
        assert_eq!(title, "Doodle");
        assert_eq!((back.width, back.height), (200, 130));
        assert_eq!(back.layers.len(), 2);
        assert_eq!(back.layers[1].props, s.layers[1].props);
        assert_eq!(back.layers[0].raster.get_pixel(5, 7), Rgba8::new(10, 20, 30, 255));
        assert_eq!(back.layers[1].raster.get_pixel(150, 100), Rgba8::new(1, 2, 3, 4));
        assert_eq!(back.layers[1].raster.get_pixel(0, 0).a, 0);
        assert_eq!(back.next_layer_id, id + 1);
    }

    /// Only the masked tiles come back; the untouched corner of the box is not
    /// in the patch at all.
    #[test]
    fn patch_carries_only_the_masked_tiles() {
        let mut s = DocState::new(300, 300, None);
        let r = &mut s.layers[0].raster;
        r.set_pixel(10, 10, Rgba8::new(255, 0, 0, 255)); // tile (0,0)
        r.set_pixel(200, 200, Rgba8::new(0, 255, 0, 255)); // tile (3,3)
        r.set_pixel(200, 10, Rgba8::new(0, 0, 255, 255)); // tile (3,0): not sent
        let txn = r.tiles_x() as usize;
        let bytes = encode_patch(&s, 1, &[0, 3 * txn + 3]).unwrap();
        let p = decode_patch(&bytes).unwrap();
        assert_eq!(p.layer, 1);
        assert_eq!(p.tiles.len(), 2);
        let (i0, t0) = &p.tiles[0];
        assert_eq!(*i0, 0);
        assert_eq!(t0.as_ref().unwrap().get(10, 10), Rgba8::new(255, 0, 0, 255));
        let (i1, t1) = &p.tiles[1];
        assert_eq!(*i1, 3 * txn + 3);
        assert_eq!(t1.as_ref().unwrap().get(200 - 192, 200 - 192), Rgba8::new(0, 255, 0, 255));
        assert!(!p.tiles.iter().any(|(i, _)| *i == 3), "the unmasked tile is not delivered");
    }

    /// A tile that became transparent arrives as `None`, so the receiver frees
    /// it instead of keeping a zero tile around.
    #[test]
    fn a_cleared_tile_arrives_empty() {
        let s = DocState::new(64, 64, None);
        let bytes = encode_patch(&s, 1, &[0]).unwrap();
        let p = decode_patch(&bytes).unwrap();
        assert!(p.tiles[0].1.is_none());
    }

    #[test]
    fn layers_round_trip() {
        let mut s = DocState::new(10, 10, None);
        s.add_layer("Top", None);
        s.layers[1].props.name = "Renamed".into();
        let h = decode_layers(&encode_layers(&s)).unwrap();
        assert_eq!(h.layers.len(), 2);
        assert_eq!(h.layers[1].name, "Renamed");
        assert_eq!(h.active, s.active_layer().props.id);
    }
}

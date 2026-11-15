//! Deferred on-demand MVT geometry and property index for tile point/box queries.

#![deny(missing_docs)]

use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashSet;

use super::{properties_bytes, IndexedGeometry};
use crate::style::{feature_state::SourceFeatureState, source::PromoteId};

/// Compact raw-MVT tile query index that defers decoding `IndexedGeometry<f64>` and feature
/// property maps until a point or box query actually inspects the tile.
#[derive(Debug)]
pub struct LazyMvtIndex {
    mvt_bytes: Arc<[u8]>,
    compressed: bool,
    promote_id: Option<PromoteId>,
    feature_state: Option<Arc<SourceFeatureState>>,
    materialized: OnceLock<Vec<IndexedGeometry<f64>>>,
}

impl Clone for LazyMvtIndex {
    fn clone(&self) -> Self {
        let materialized = OnceLock::new();
        if let Some(list) = self.materialized.get() {
            let _ = materialized.set(list.clone());
        }
        Self {
            mvt_bytes: Arc::clone(&self.mvt_bytes),
            compressed: self.compressed,
            promote_id: self.promote_id.clone(),
            feature_state: self.feature_state.clone(),
            materialized,
        }
    }
}

impl LazyMvtIndex {
    /// Packs raw MVT bytes for storage (LZ4-compressing on `wasm32` so resident tile indices stay
    /// compact in linear memory without linking a deflate compressor).
    pub(crate) fn pack_slice(raw: &[u8]) -> (Arc<[u8]>, bool) {
        #[cfg(target_arch = "wasm32")]
        if let Some(packed) = compress_lz4_block(raw) {
            return (Arc::from(packed), true);
        }
        (Arc::from(raw), false)
    }

    /// Packs owned raw MVT bytes without copying on native and deflating on `wasm32`.
    pub(crate) fn pack_vec(raw: Vec<u8>) -> (Arc<[u8]>, bool) {
        #[cfg(target_arch = "wasm32")]
        {
            Self::pack_slice(&raw)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            (Arc::from(raw), false)
        }
    }

    /// Creates an unmaterialized index over raw MVT bytes.
    pub fn new(mvt_bytes: Arc<[u8]>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let (mvt_bytes, compressed) = Self::pack_slice(&mvt_bytes);
        #[cfg(not(target_arch = "wasm32"))]
        let compressed = false;
        Self {
            mvt_bytes,
            compressed,
            promote_id: None,
            feature_state: None,
            materialized: OnceLock::new(),
        }
    }

    /// Reconstructs an unmaterialized index from wire bytes and their compression flag.
    pub(crate) fn from_packed(mvt_bytes: Arc<[u8]>, compressed: bool) -> Self {
        Self {
            mvt_bytes,
            compressed,
            promote_id: None,
            feature_state: None,
            materialized: OnceLock::new(),
        }
    }

    /// Creates an unmaterialized index from pre-packed storage and active `feature-state` snapshot.
    pub(crate) fn from_packed_with_state(
        mvt_bytes: Arc<[u8]>,
        compressed: bool,
        promote_id: Option<PromoteId>,
        feature_state: Option<SourceFeatureState>,
    ) -> Self {
        Self {
            mvt_bytes,
            compressed,
            promote_id,
            feature_state: feature_state.map(Arc::new),
            materialized: OnceLock::new(),
        }
    }

    /// Underlying stored MVT byte slice and whether it is deflate-compressed.
    pub(crate) fn wire_bytes(&self) -> (&Arc<[u8]>, bool) {
        (&self.mvt_bytes, self.compressed)
    }

    /// Underlying stored MVT protobuf slice.
    pub fn mvt_bytes(&self) -> &Arc<[u8]> {
        &self.mvt_bytes
    }

    /// Materializes and returns all indexed geometries in feature and part order.
    pub fn geometries(&self) -> &[IndexedGeometry<f64>] {
        self.materialized.get_or_init(|| {
            if self.compressed {
                if let Some(raw) = decompress_lz4_block(&self.mvt_bytes) {
                    return crate::vector::index_mvt_bytes(
                        &raw,
                        self.promote_id.as_ref(),
                        self.feature_state.as_ref(),
                    );
                }
                return Vec::new();
            }
            crate::vector::index_mvt_bytes(
                &self.mvt_bytes,
                self.promote_id.as_ref(),
                self.feature_state.as_ref(),
            )
        })
    }

    pub(crate) fn materialized_slice(&self) -> Option<&[IndexedGeometry<f64>]> {
        self.materialized.get().map(Vec::as_slice)
    }

    pub(crate) fn materialized_mut(&mut self) -> Option<&mut [IndexedGeometry<f64>]> {
        self.materialized.get_mut().map(Vec::as_mut_slice)
    }

    pub(crate) fn update_unmaterialized_state(
        &mut self,
        promote_id: Option<PromoteId>,
        feature_state: Option<Arc<SourceFeatureState>>,
    ) {
        self.promote_id = promote_id;
        self.feature_state = feature_state;
    }

    /// Approximate resident heap footprint in bytes.
    pub fn approximate_bytes(&self) -> usize {
        let mut total = self.mvt_bytes.len();
        if let Some(list) = self.materialized.get() {
            let mut shared = FxHashSet::default();
            for geometry in list {
                total += geometry.geometry_bytes();
                if shared.insert(Arc::as_ptr(&geometry.properties)) {
                    total += properties_bytes(&geometry.properties);
                }
            }
        }
        total
    }
}

const MAX_DECOMPRESSED_MVT_BYTES: usize = 64 * 1024 * 1024;

fn push_lz4_extra(out: &mut Vec<u8>, mut len: usize) {
    if len < 15 {
        return;
    }
    len -= 15;
    while len >= 255 {
        out.push(255);
        len -= 255;
    }
    out.push(len as u8);
}

#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
fn compress_lz4_block(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 16 {
        return None;
    }
    let orig_len = u32::try_from(raw.len()).ok()?;
    let mut out = Vec::with_capacity(raw.len() / 2 + 16);
    out.extend_from_slice(&orig_len.to_le_bytes());
    let mut table = [0_u32; 4096];
    let mut anchor = 0_usize;
    let mut ip = 0_usize;
    let limit = raw.len().saturating_sub(4);

    while ip <= limit {
        let b0 = *raw.get(ip)?;
        let b1 = *raw.get(ip + 1)?;
        let b2 = *raw.get(ip + 2)?;
        let b3 = *raw.get(ip + 3)?;
        let seq = u32::from_le_bytes([b0, b1, b2, b3]);
        let hash = (seq.wrapping_mul(2_654_435_761) >> 20) as usize;
        let prev = table[hash];
        table[hash] = (ip as u32).saturating_add(1);
        if prev > 0 {
            let ref_pos = (prev - 1) as usize;
            let offset = ip.saturating_sub(ref_pos);
            if (1..=65535).contains(&offset) && raw.get(ref_pos..ref_pos + 4) == raw.get(ip..ip + 4)
            {
                let mut match_len = 4_usize;
                while ip + match_len < raw.len()
                    && raw.get(ref_pos + match_len) == raw.get(ip + match_len)
                {
                    match_len += 1;
                }
                let lit_len = ip - anchor;
                let match_extra = match_len - 4;
                out.push(((lit_len.min(15) as u8) << 4) | (match_extra.min(15) as u8));
                push_lz4_extra(&mut out, lit_len);
                out.extend_from_slice(raw.get(anchor..ip)?);
                out.extend_from_slice(&(offset as u16).to_le_bytes());
                push_lz4_extra(&mut out, match_extra);
                ip += match_len;
                anchor = ip;
                if out.len() >= raw.len() {
                    return None;
                }
                continue;
            }
        }
        ip += 1 + ((ip - anchor) >> 5);
    }

    let lit_len = raw.len() - anchor;
    out.push((lit_len.min(15) as u8) << 4);
    push_lz4_extra(&mut out, lit_len);
    out.extend_from_slice(raw.get(anchor..)?);
    if out.len() < raw.len() {
        out.shrink_to_fit();
        Some(out)
    } else {
        None
    }
}

fn read_lz4_extra(packed: &[u8], pos: &mut usize, base: usize) -> Option<usize> {
    if base < 15 {
        return Some(base);
    }
    let mut total = base;
    loop {
        let byte = *packed.get(*pos)?;
        *pos += 1;
        total = total.checked_add(usize::from(byte))?;
        if byte < 255 {
            return Some(total);
        }
    }
}

fn decompress_lz4_block(packed: &[u8]) -> Option<Vec<u8>> {
    let hdr = packed.get(..4)?;
    let expected = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
    if expected > MAX_DECOMPRESSED_MVT_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(expected);
    let mut pos = 4_usize;
    while pos < packed.len() {
        let token = *packed.get(pos)?;
        pos += 1;
        let lit_len = read_lz4_extra(packed, &mut pos, usize::from(token >> 4))?;
        let lit_end = pos.checked_add(lit_len)?;
        let literals = packed.get(pos..lit_end)?;
        if out.len().checked_add(lit_len)? > expected {
            return None;
        }
        out.extend_from_slice(literals);
        pos = lit_end;
        if pos == packed.len() {
            break;
        }
        let lo = *packed.get(pos)?;
        let hi = *packed.get(pos + 1)?;
        pos += 2;
        let offset = usize::from(u16::from_le_bytes([lo, hi]));
        if offset == 0 || offset > out.len() {
            return None;
        }
        let match_len =
            read_lz4_extra(packed, &mut pos, usize::from(token & 0x0F))?.checked_add(4)?;
        if out.len().checked_add(match_len)? > expected {
            return None;
        }
        if offset >= match_len {
            let start = out.len() - offset;
            out.extend_from_within(start..start + match_len);
        } else {
            for _ in 0..match_len {
                let b = *out.get(out.len() - offset)?;
                out.push(b);
            }
        }
    }
    (out.len() == expected).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{compress_lz4_block, decompress_lz4_block};

    #[test]
    fn lz4_block_round_trips_repetitive_and_rle_streams() {
        let mut sample = Vec::new();
        for i in 0..240_u32 {
            sample.extend_from_slice(b"layer:building\x12\x0brender_height\x1a");
            sample.extend_from_slice(&i.to_le_bytes());
            sample.extend_from_slice(&[0xAA; 24]);
        }
        let packed = compress_lz4_block(&sample);
        assert!(packed.is_some());
        if let Some(packed) = packed {
            assert!(packed.len() * 2 < sample.len());
            assert_eq!(decompress_lz4_block(&packed).as_deref(), Some(&sample[..]));
        }
    }
}

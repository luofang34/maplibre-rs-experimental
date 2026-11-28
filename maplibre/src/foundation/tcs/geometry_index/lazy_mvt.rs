//! Deferred on-demand MVT geometry and property index for tile point/box queries.

#![deny(missing_docs)]

use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashSet;

use super::{properties_bytes, IndexedGeometry};
use crate::{
    coords::WorldTileCoords,
    style::{feature_state::SourceFeatureState, source::PromoteId},
};

/// Callback decoding raw MVT/MLT bytes into indexed query geometries on demand.
pub(crate) type MvtMaterializer = fn(
    &[u8],
    Option<(WorldTileCoords, WorldTileCoords)>,
    Option<&PromoteId>,
    Option<&Arc<SourceFeatureState>>,
) -> Vec<IndexedGeometry<f64>>;

static MVT_MATERIALIZER: OnceLock<MvtMaterializer> = OnceLock::new();

/// Registers the vector tile geometry index materializer used by [`LazyMvtIndex::geometries`].
pub(crate) fn register_mvt_materializer(materialize: MvtMaterializer) {
    MVT_MATERIALIZER.get_or_init(|| materialize);
}

/// Serialized wire representation of an unmaterialized [`LazyMvtIndex`].
#[derive(Clone, Copy)]
pub struct LazyMvtWire<'a> {
    /// Stored raw or packed MVT bytes.
    pub bytes: &'a [u8],
    /// Whether [`Self::bytes`] is compressed.
    pub compressed: bool,
}

/// Compact raw-MVT tile query index that defers decoding `IndexedGeometry<f64>` and feature
/// property maps until a point or box query actually inspects the tile.
#[derive(Debug)]
pub struct LazyMvtIndex {
    mvt_bytes: Arc<[u8]>,
    compressed: bool,
    rescale: Option<(WorldTileCoords, WorldTileCoords)>,
    promote_id: Option<PromoteId>,
    feature_state: Option<Arc<SourceFeatureState>>,
    materialized: OnceLock<Vec<IndexedGeometry<f64>>>,
}

impl Clone for LazyMvtIndex {
    fn clone(&self) -> Self {
        let materialized = OnceLock::new();
        if let Some(list) = self.materialized.get() {
            drop(materialized.set(list.clone()));
        }
        Self {
            mvt_bytes: Arc::clone(&self.mvt_bytes),
            compressed: self.compressed,
            rescale: self.rescale,
            promote_id: self.promote_id.clone(),
            feature_state: self.feature_state.clone(),
            materialized,
        }
    }
}

impl LazyMvtIndex {
    /// Compresses raw MVT bytes when compression shrinks the payload.
    pub(crate) fn pack_slice(raw: &[u8]) -> (Arc<[u8]>, bool) {
        if let Some(packed) = compress_packed(raw) {
            return (Arc::from(packed), true);
        }
        (Arc::from(raw), false)
    }

    /// Packs owned raw MVT bytes, compressing when compression shrinks the payload.
    pub(crate) fn pack_vec(raw: Vec<u8>) -> (Arc<[u8]>, bool) {
        if let Some(packed) = compress_packed(&raw) {
            return (Arc::from(packed), true);
        }
        (Arc::from(raw), false)
    }

    /// Decompresses packed MVT bytes produced by [`Self::pack_slice`] / [`Self::pack_vec`].
    pub(crate) fn decompress_packed(packed: &[u8]) -> Option<Vec<u8>> {
        decompress_packed_bytes(packed)
    }

    /// Creates an unmaterialized index over raw MVT bytes.
    pub fn new(mvt_bytes: Arc<[u8]>) -> Self {
        let (mvt_bytes, compressed) = Self::pack_slice(&mvt_bytes);
        Self {
            mvt_bytes,
            compressed,
            rescale: None,
            promote_id: None,
            feature_state: None,
            materialized: OnceLock::new(),
        }
    }

    /// Reconstructs an unmaterialized index from wire bytes and their compression flag.
    pub fn from_packed(mvt_bytes: Arc<[u8]>, compressed: bool) -> Self {
        Self {
            mvt_bytes,
            compressed,
            rescale: None,
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
            rescale: None,
            promote_id,
            feature_state: feature_state.map(Arc::new),
            materialized: OnceLock::new(),
        }
    }

    /// Attaches an `(ancestor, descendant)` rescaling pair so dormant overscaled indices clip and
    /// rescale from the shared ancestor MVT bytes only when a feature query materializes them.
    pub fn with_rescale(mut self, rescale: Option<(WorldTileCoords, WorldTileCoords)>) -> Self {
        self.rescale = rescale;
        self
    }

    /// Optional `(ancestor, descendant)` coordinates to rescale raw bytes on materialization.
    pub fn rescale(&self) -> Option<(WorldTileCoords, WorldTileCoords)> {
        self.rescale
    }

    /// Underlying stored MVT bytes and compression flag as a borrowed view.
    #[inline]
    pub fn wire(&self) -> LazyMvtWire<'_> {
        LazyMvtWire {
            bytes: &self.mvt_bytes,
            compressed: self.compressed,
        }
    }

    /// Underlying stored MVT byte slice and whether it is deflate-compressed.
    #[cfg(test)]
    pub fn wire_bytes(&self) -> (&Arc<[u8]>, bool) {
        (&self.mvt_bytes, self.compressed)
    }

    /// Underlying stored MVT protobuf slice.
    pub fn mvt_bytes(&self) -> &Arc<[u8]> {
        &self.mvt_bytes
    }

    /// Materializes and returns all indexed geometries using the provided `materialize` callback.
    pub(crate) fn geometries_with(&self, materialize: MvtMaterializer) -> &[IndexedGeometry<f64>] {
        self.materialized.get_or_init(|| {
            if self.compressed {
                if let Some(raw) = Self::decompress_packed(&self.mvt_bytes) {
                    return materialize(
                        &raw,
                        self.rescale,
                        self.promote_id.as_ref(),
                        self.feature_state.as_ref(),
                    );
                }
                return Vec::new();
            }
            materialize(
                &self.mvt_bytes,
                self.rescale,
                self.promote_id.as_ref(),
                self.feature_state.as_ref(),
            )
        })
    }

    /// Materializes and returns all indexed geometries in feature and part order.
    pub fn geometries(&self) -> &[IndexedGeometry<f64>] {
        let Some(&materialize) = MVT_MATERIALIZER.get() else {
            return &[];
        };
        self.geometries_with(materialize)
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
        if self.materialized.get().is_some() {
            return;
        }
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
#[cfg(not(target_arch = "wasm32"))]
const DEFLATE_MAGIC: [u8; 4] = *b"ZMVT";

#[cfg(not(target_arch = "wasm32"))]
fn compress_packed(raw: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    if raw.len() < 16 {
        return None;
    }
    let orig_len = u32::try_from(raw.len()).ok()?;
    let mut out = Vec::with_capacity(raw.len() / 2 + 16);
    out.extend_from_slice(&DEFLATE_MAGIC);
    out.extend_from_slice(&orig_len.to_le_bytes());
    let mut encoder = flate2::write::DeflateEncoder::new(out, flate2::Compression::fast());
    encoder.write_all(raw).ok()?;
    let mut compressed = encoder.finish().ok()?;
    if compressed.len() < raw.len() {
        compressed.shrink_to_fit();
        Some(compressed)
    } else {
        None
    }
}

#[cfg(target_arch = "wasm32")]
fn compress_packed(raw: &[u8]) -> Option<Vec<u8>> {
    compress_lz4_block(raw)
}

fn decompress_packed_bytes(packed: &[u8]) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    if packed.starts_with(&DEFLATE_MAGIC) {
        use std::io::Read as _;
        let expected = read_u32_le_prefix(packed.get(4..8)?)?;
        if expected > MAX_DECOMPRESSED_MVT_BYTES {
            return None;
        }
        let payload = packed.get(8..)?;
        let mut out = Vec::with_capacity(expected);
        let decoder = flate2::read::DeflateDecoder::new(payload);
        decoder
            .take((expected as u64) + 1)
            .read_to_end(&mut out)
            .ok()?;
        return (out.len() == expected).then_some(out);
    }
    decompress_lz4_block(packed)
}

#[cfg(any(target_arch = "wasm32", test))]
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

#[cfg(any(target_arch = "wasm32", test))]
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
        let prev = match table.get_mut(hash) {
            Some(slot) => {
                let prev = *slot;
                *slot = (ip as u32).saturating_add(1);
                prev
            }
            None => 0,
        };
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

fn read_u32_le_prefix(bytes: &[u8]) -> Option<usize> {
    let &[b0, b1, b2, b3] = bytes.get(..4)? else {
        return None;
    };
    Some(u32::from_le_bytes([b0, b1, b2, b3]) as usize)
}

fn read_lz4_offset(packed: &[u8], pos: &mut usize) -> Option<usize> {
    let lo = *packed.get(*pos)?;
    let hi = *packed.get(*pos + 1)?;
    *pos += 2;
    Some(usize::from(u16::from_le_bytes([lo, hi])))
}

fn checked_lz4_len(current: usize, add: usize, expected: usize) -> Option<()> {
    (current.checked_add(add)? <= expected).then_some(())
}

fn copy_lz4_match(out: &mut Vec<u8>, offset: usize, match_len: usize) -> Option<()> {
    if offset >= match_len {
        let start = out.len() - offset;
        out.extend_from_within(start..start + match_len);
        return Some(());
    }
    for _ in 0..match_len {
        let b = *out.get(out.len() - offset)?;
        out.push(b);
    }
    Some(())
}

fn decompress_lz4_block(packed: &[u8]) -> Option<Vec<u8>> {
    let expected = read_u32_le_prefix(packed)?;
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
        checked_lz4_len(out.len(), lit_len, expected)?;
        out.extend_from_slice(literals);
        pos = lit_end;
        if pos == packed.len() {
            break;
        }
        let offset = read_lz4_offset(packed, &mut pos)?;
        if offset == 0 || offset > out.len() {
            return None;
        }
        let match_len =
            read_lz4_extra(packed, &mut pos, usize::from(token & 0x0F))?.checked_add(4)?;
        checked_lz4_len(out.len(), match_len, expected)?;
        copy_lz4_match(&mut out, offset, match_len)?;
    }
    (out.len() == expected).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{compress_lz4_block, decompress_lz4_block, LazyMvtIndex};

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
            assert_eq!(
                LazyMvtIndex::decompress_packed(&packed).as_deref(),
                Some(&sample[..])
            );
        }
        let (native_packed, compressed) = LazyMvtIndex::pack_slice(&sample);
        assert!(compressed);
        assert!(native_packed.len() * 2 < sample.len());
        assert_eq!(
            LazyMvtIndex::decompress_packed(&native_packed).as_deref(),
            Some(&sample[..])
        );
    }
}

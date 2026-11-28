//! Symbol geometry, collision ranges and assets crossing the worker boundary.

use bytemuck::Zeroable;

use super::*;

#[cfg(any(test, feature = "renderer"))]
pub(super) mod js_wire;
#[cfg(test)]
mod tests;

const SYMBOL_HEADER_BYTES: usize = 40;

#[derive(Clone, Copy)]
pub(super) struct SymbolHeader {
    coords: WorldTileCoords,
    has_atlas: bool,
    usable_indices: u32,
    vert_count: usize,
    idx_count: usize,
    layer_name_len: usize,
    style_id_len: usize,
    feat_len: usize,
    atlas_len: usize,
}

pub(super) fn parse_symbol_header(bytes: &[u8], total_len: usize) -> Option<SymbolHeader> {
    if bytes.len() < SYMBOL_HEADER_BYTES || total_len < SYMBOL_HEADER_BYTES {
        return None;
    }
    let coords = read_coords_at(bytes, 0)?;
    let has_atlas = *bytes.get(9)? != 0;
    let usable_indices = read_u32_le(bytes, 12)?;
    let vert_count = read_u32_le(bytes, 16)? as usize;
    let idx_count = read_u32_le(bytes, 20)? as usize;
    let layer_name_len = read_u32_le(bytes, 24)? as usize;
    let style_id_len = read_u32_le(bytes, 28)? as usize;
    let feat_len = read_u32_le(bytes, 32)? as usize;
    let atlas_len = read_u32_le(bytes, 36)? as usize;
    let body = vert_count
        .checked_mul(std::mem::size_of::<ShaderSymbolVertex>())?
        .checked_add(idx_count.checked_mul(4)?)?
        .checked_add(layer_name_len)?
        .checked_add(style_id_len)?
        .checked_add(feat_len)?
        .checked_add(atlas_len)?;
    if SYMBOL_HEADER_BYTES.checked_add(body)? > total_len {
        return None;
    }
    Some(SymbolHeader {
        coords,
        has_atlas,
        usable_indices,
        vert_count,
        idx_count,
        layer_name_len,
        style_id_len,
        feat_len,
        atlas_len,
    })
}

impl SymbolLayerTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::SymbolLayerTessellated
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        let parts = buffer.into_raw_parts();
        let usable_indices = parts.usable_indices;
        let raw_vertices = parts.vertices;
        let raw_indices = parts.indices;
        let vert_bytes = raw_vertices.len() * std::mem::size_of::<ShaderSymbolVertex>();
        let idx_bytes = raw_indices.len() * 4;
        let layer_bytes = layer_data.name.as_bytes();
        let style_bytes = style_layer_id.as_bytes();
        let feature_bytes = maplibre::sdf::assets::wire::encode_symbol_features(&features);
        drop(features);
        let atlas_bytes = atlas.as_ref().map(|a| encode_symbol_atlas(coords, a));
        let atlas_len = atlas_bytes.as_ref().map_or(0, Vec::len);

        let total = SYMBOL_HEADER_BYTES
            + vert_bytes
            + idx_bytes
            + layer_bytes.len()
            + style_bytes.len()
            + feature_bytes.len()
            + atlas_len;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&coords.x.to_le_bytes());
        out.extend_from_slice(&coords.y.to_le_bytes());
        out.push(u8::from(coords.z));
        out.push(u8::from(atlas_bytes.is_some()));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&usable_indices.to_le_bytes());
        out.extend_from_slice(&(raw_vertices.len() as u32).to_le_bytes());
        out.extend_from_slice(&(raw_indices.len() as u32).to_le_bytes());
        out.extend_from_slice(&(layer_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(style_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(feature_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(atlas_len as u32).to_le_bytes());

        out.extend_from_slice(bytemuck::cast_slice(&raw_vertices));
        drop(raw_vertices);
        out.extend_from_slice(bytemuck::cast_slice(&raw_indices));
        drop(raw_indices);
        out.extend_from_slice(layer_bytes);
        out.extend_from_slice(style_bytes);
        out.extend_from_slice(&feature_bytes);
        drop(feature_bytes);
        if let Some(atlas) = atlas_bytes {
            out.extend_from_slice(&atlas);
        }
        FlatBufferTransferable::from_vec(WebMessageTag::SymbolLayerTessellated, out, 0)
    }

    fn coords(&self) -> WorldTileCoords {
        if let Some(sym) = &self.unpacked_symbol {
            return sym.coords;
        }
        self.data()
            .get(..SYMBOL_HEADER_BYTES)
            .and_then(|bytes| read_coords_at(bytes, 0))
            .unwrap_or_default()
    }

    fn is_empty(&self) -> bool {
        if let Some(sym) = &self.unpacked_symbol {
            return sym.buffer.usable_indices == 0;
        }
        self.data()
            .get(..SYMBOL_HEADER_BYTES)
            .and_then(|bytes| read_u32_le(bytes, 12))
            .is_none_or(|usable| usable == 0)
    }

    fn to_bucket(mut self) -> SymbolLayerData {
        if let Some(sym) = self.unpacked_symbol.take() {
            return *sym;
        }
        let coords = SymbolLayerTessellated::coords(&self);
        decode_symbol_bucket(self.data, self.start).unwrap_or_else(|| SymbolLayerData {
            coords,
            source_layer: String::new(),
            style_layer_id: String::new(),
            buffer: OverAlignedVertexBuffer::empty(),
            features: Vec::new(),
            atlas: None,
        })
    }
}

type SymbolBody = (
    SymbolHeader,
    OverAlignedVertexBuffer<ShaderSymbolVertex, u32>,
    String,
    String,
    Vec<Feature>,
    usize,
);

fn parse_symbol_body(bytes: &[u8]) -> Option<SymbolBody> {
    let hdr = parse_symbol_header(bytes, bytes.len())?;
    let mut cursor = SYMBOL_HEADER_BYTES;
    let take = |cur: &mut usize, len: usize| -> Option<&[u8]> {
        let end = cur.checked_add(len)?;
        let slice = bytes.get(*cur..end)?;
        *cur = end;
        Some(slice)
    };

    let vert_bytes_len = hdr.vert_count * std::mem::size_of::<ShaderSymbolVertex>();
    let mut vertices = vec![ShaderSymbolVertex::zeroed(); hdr.vert_count];
    bytemuck::cast_slice_mut(&mut vertices).copy_from_slice(take(&mut cursor, vert_bytes_len)?);

    let idx_bytes_len = hdr.idx_count * 4;
    let mut indices = vec![0_u32; hdr.idx_count];
    bytemuck::cast_slice_mut(&mut indices).copy_from_slice(take(&mut cursor, idx_bytes_len)?);

    let source_layer = std::str::from_utf8(take(&mut cursor, hdr.layer_name_len)?)
        .unwrap_or_default()
        .to_owned();
    let style_layer_id = if hdr.style_id_len == 0 {
        source_layer.clone()
    } else {
        std::str::from_utf8(take(&mut cursor, hdr.style_id_len)?)
            .unwrap_or_default()
            .to_owned()
    };
    let features =
        maplibre::sdf::assets::wire::decode_symbol_features(take(&mut cursor, hdr.feat_len)?)
            .unwrap_or_else(|| {
                tracing::error!("invalid symbol features");
                Vec::new()
            });
    Some((
        hdr,
        OverAlignedVertexBuffer::from_vecs(vertices, indices, hdr.usable_indices),
        source_layer,
        style_layer_id,
        features,
        cursor,
    ))
}

fn decode_symbol_bucket(owned_data: Vec<u8>, start: usize) -> Option<SymbolLayerData> {
    let bytes = owned_data.get(start..)?;
    let (hdr, buffer, source_layer, style_layer_id, features, cursor) = parse_symbol_body(bytes)?;
    let atlas = hdr
        .has_atlas
        .then(|| decode_symbol_atlas_owned(hdr.coords, owned_data, start + cursor, hdr.atlas_len))
        .flatten();

    Some(SymbolLayerData {
        coords: hdr.coords,
        source_layer,
        style_layer_id,
        buffer,
        features,
        atlas,
    })
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn decode_symbol_bucket_from_bytes(bytes: &[u8]) -> Option<SymbolLayerData> {
    let (hdr, buffer, source_layer, style_layer_id, features, cursor) = parse_symbol_body(bytes)?;
    let atlas = hdr
        .has_atlas
        .then(|| {
            let end = cursor.checked_add(hdr.atlas_len)?;
            decode_symbol_atlas_slice(hdr.coords, bytes.get(cursor..end)?)
        })
        .flatten();

    Some(SymbolLayerData {
        coords: hdr.coords,
        source_layer,
        style_layer_id,
        buffer,
        features,
        atlas,
    })
}

#[cfg(any(test, feature = "renderer"))]
fn decode_symbol_atlas_slice(
    coords: WorldTileCoords,
    bytes: &[u8],
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    let first_word = u32::from_le_bytes(*bytes.first_chunk::<4>()?);
    if first_word == ATLAS_WIRE_MAGIC {
        let hdr24 = bytes.first_chunk::<24>()?;
        let kind = u32::from_le_bytes([hdr24[4], hdr24[5], hdr24[6], hdr24[7]]);
        let width = u32::from_le_bytes([hdr24[8], hdr24[9], hdr24[10], hdr24[11]]);
        let height = u32::from_le_bytes([hdr24[12], hdr24[13], hdr24[14], hdr24[15]]);
        let ptr_id = u64::from_le_bytes([
            hdr24[16], hdr24[17], hdr24[18], hdr24[19], hdr24[20], hdr24[21], hdr24[22], hdr24[23],
        ]);
        let key = (coords, width, height, ptr_id);
        if let Some(hit) = check_atlas_cache(&key) {
            return Some(hit);
        }
        if kind == ATLAS_WIRE_REF {
            return None;
        }
        let pixels = bytes.get(24..)?;
        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if pixels.len() != expected_len {
            return None;
        }
        return Some(insert_atlas_cache(key, [width, height], pixels.to_vec()));
    }
    atlas_from_wire_bytes(coords, bytes)
}

pub(super) const ATLAS_WIRE_MAGIC: u32 = u32::MAX;
const ATLAS_WIRE_FULL: u32 = 1;
pub(super) const ATLAS_WIRE_REF: u32 = 2;
const RECENT_ATLAS_RING_CAP: usize = 1;

pub(super) type AtlasCacheKey = (WorldTileCoords, u32, u32, u64);

struct MainAtlasCache {
    weak: rustc_hash::FxHashMap<AtlasCacheKey, std::sync::Weak<maplibre::sdf::assets::SymbolAtlas>>,
    recent: std::collections::VecDeque<(
        AtlasCacheKey,
        std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>,
    )>,
}

thread_local! {
    static ATLAS_CACHE: std::cell::RefCell<MainAtlasCache> =
        std::cell::RefCell::new(MainAtlasCache {
            weak: rustc_hash::FxHashMap::default(),
            recent: std::collections::VecDeque::with_capacity(RECENT_ATLAS_RING_CAP),
        });
}

pub(super) fn check_atlas_cache(
    key: &AtlasCacheKey,
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    ATLAS_CACHE.with(|cache| {
        let cache = cache.borrow();
        if let Some((_, arc)) = cache.recent.iter().find(|(k, _)| k == key) {
            return Some(std::sync::Arc::clone(arc));
        }
        cache.weak.get(key).and_then(std::sync::Weak::upgrade)
    })
}

pub(super) fn insert_atlas_cache(
    key: AtlasCacheKey,
    size: [u32; 2],
    pixels: Vec<u8>,
) -> std::sync::Arc<maplibre::sdf::assets::SymbolAtlas> {
    ATLAS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.weak.retain(|_, weak| weak.strong_count() > 0);
        let arc = std::sync::Arc::new(maplibre::sdf::assets::SymbolAtlas::with_wasm_staging(
            size, pixels,
        ));
        cache.weak.insert(key, std::sync::Arc::downgrade(&arc));
        if cache.recent.len() >= RECENT_ATLAS_RING_CAP {
            cache.recent.pop_front();
        }
        cache.recent.push_back((key, std::sync::Arc::clone(&arc)));
        arc
    })
}

#[cfg(any(test, feature = "renderer"))]
pub(crate) fn clear_atlas_cache() {
    ATLAS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.recent.clear();
        cache.weak.retain(|_, weak| weak.strong_count() > 0);
    });
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn atlas_from_wire_bytes(
    coords: WorldTileCoords,
    bytes: &[u8],
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    let (hdr, pixels) = bytes.split_first_chunk::<8>()?;
    let first_word = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    if first_word == ATLAS_WIRE_MAGIC {
        return None;
    }
    let width = first_word;
    let height = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    if pixels.len()
        != (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
    {
        return None;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    pixels.hash(&mut hasher);
    let key = (coords, width, height, hasher.finish());
    if let Some(existing) = check_atlas_cache(&key) {
        return Some(existing);
    }
    Some(insert_atlas_cache(key, [width, height], pixels.to_vec()))
}

fn encode_symbol_atlas(
    coords: WorldTileCoords,
    atlas: &std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>,
) -> Vec<u8> {
    type SentKey = (WorldTileCoords, usize);
    type SentMap =
        rustc_hash::FxHashMap<SentKey, std::sync::Weak<maplibre::sdf::assets::SymbolAtlas>>;
    thread_local! {
        static SENT_ATLASES: std::cell::RefCell<SentMap> =
            std::cell::RefCell::new(rustc_hash::FxHashMap::default());
    }
    let ptr_usize = std::sync::Arc::as_ptr(atlas) as usize;
    let ptr_id = ptr_usize as u64;
    let already_sent = SENT_ATLASES.with(|sent| {
        let mut sent = sent.borrow_mut();
        if let Some(existing) = sent
            .get(&(coords, ptr_usize))
            .and_then(std::sync::Weak::upgrade)
        {
            if std::sync::Arc::ptr_eq(&existing, atlas) {
                return true;
            }
        }
        if sent.len() >= 64 {
            sent.retain(|_, weak| weak.strong_count() > 0);
        }
        sent.insert((coords, ptr_usize), std::sync::Arc::downgrade(atlas));
        false
    });
    let (atlas_w, atlas_h) = atlas.dimensions();
    if already_sent {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&ATLAS_WIRE_MAGIC.to_le_bytes());
        out.extend_from_slice(&ATLAS_WIRE_REF.to_le_bytes());
        out.extend_from_slice(&atlas_w.to_le_bytes());
        out.extend_from_slice(&atlas_h.to_le_bytes());
        out.extend_from_slice(&ptr_id.to_le_bytes());
        return out;
    }
    atlas.with_pixels(|pixels| {
        let mut out = Vec::with_capacity(24 + pixels.len());
        out.extend_from_slice(&ATLAS_WIRE_MAGIC.to_le_bytes());
        out.extend_from_slice(&ATLAS_WIRE_FULL.to_le_bytes());
        out.extend_from_slice(&atlas_w.to_le_bytes());
        out.extend_from_slice(&atlas_h.to_le_bytes());
        out.extend_from_slice(&ptr_id.to_le_bytes());
        out.extend_from_slice(pixels);
        out
    })
}

fn decode_symbol_atlas_owned(
    coords: WorldTileCoords,
    mut owned_data: Vec<u8>,
    offset: usize,
    len: usize,
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    let end = offset.checked_add(len)?;
    let bytes = owned_data.get(offset..end)?;
    let (hdr8, pixels) = bytes.split_first_chunk::<8>()?;
    let first_word = u32::from_le_bytes([hdr8[0], hdr8[1], hdr8[2], hdr8[3]]);
    if first_word == ATLAS_WIRE_MAGIC {
        let hdr24 = bytes.first_chunk::<24>()?;
        let kind = u32::from_le_bytes([hdr24[4], hdr24[5], hdr24[6], hdr24[7]]);
        let width = u32::from_le_bytes([hdr24[8], hdr24[9], hdr24[10], hdr24[11]]);
        let height = u32::from_le_bytes([hdr24[12], hdr24[13], hdr24[14], hdr24[15]]);
        let ptr_id = u64::from_le_bytes([
            hdr24[16], hdr24[17], hdr24[18], hdr24[19], hdr24[20], hdr24[21], hdr24[22], hdr24[23],
        ]);
        let key = (coords, width, height, ptr_id);
        if let Some(hit) = check_atlas_cache(&key) {
            return Some(hit);
        }
        if kind == ATLAS_WIRE_REF {
            return None;
        }
        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if len.saturating_sub(24) != expected_len {
            return None;
        }
        owned_data.truncate(end);
        owned_data.drain(..offset + 24);
        owned_data.shrink_to_fit();
        return Some(insert_atlas_cache(key, [width, height], owned_data));
    }
    let width = first_word;
    let height = u32::from_le_bytes([hdr8[4], hdr8[5], hdr8[6], hdr8[7]]);
    if pixels.len()
        != (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
    {
        return None;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    pixels.hash(&mut hasher);
    let key = (coords, width, height, hasher.finish());
    if let Some(existing) = check_atlas_cache(&key) {
        return Some(existing);
    }
    owned_data.truncate(end);
    owned_data.drain(..offset + 8);
    owned_data.shrink_to_fit();
    Some(insert_atlas_cache(key, [width, height], owned_data))
}

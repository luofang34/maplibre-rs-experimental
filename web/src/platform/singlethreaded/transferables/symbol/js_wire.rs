//! Direct JS `ArrayBuffer` reader for [`SymbolLayerTessellated`](maplibre::vector::SymbolLayerTessellated) payloads.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use bytemuck::Zeroable;
use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::{
    benchmarking::tessellation::OverAlignedVertexBuffer,
    coords::WorldTileCoords,
    render::shaders::ShaderSymbolVertex,
    sdf::{assets::SymbolAtlas, SymbolLayerData},
};

use super::{
    atlas_from_wire_bytes, check_atlas_cache, insert_atlas_cache, parse_symbol_header,
    ATLAS_WIRE_MAGIC, ATLAS_WIRE_REF, SYMBOL_HEADER_BYTES,
};

fn copy_js_slice(buffer: &ArrayBuffer, cur: &mut u32, dst: &mut [u8]) {
    if !dst.is_empty() {
        let len = dst.len() as u32;
        Uint8Array::new_with_byte_offset_and_length(buffer, *cur, len).copy_to(dst);
        *cur += len;
    }
}

fn decode_atlas_from_js(
    coords: WorldTileCoords,
    buffer: &ArrayBuffer,
    base: u32,
    atlas_len: usize,
) -> Option<std::sync::Arc<SymbolAtlas>> {
    if atlas_len < 8 {
        return None;
    }
    if atlas_len >= 24 {
        let mut hdr = [0_u8; 24];
        Uint8Array::new_with_byte_offset_and_length(buffer, base, 24).copy_to(&mut hdr);
        let first_word = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        if first_word == ATLAS_WIRE_MAGIC {
            let kind = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
            let width = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
            let height = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);
            let ptr_id = u64::from_le_bytes([
                hdr[16], hdr[17], hdr[18], hdr[19], hdr[20], hdr[21], hdr[22], hdr[23],
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
            if atlas_len.saturating_sub(24) != expected_len {
                return None;
            }
            let mut pixels = vec![0_u8; expected_len];
            if expected_len > 0 {
                Uint8Array::new_with_byte_offset_and_length(buffer, base + 24, expected_len as u32)
                    .copy_to(&mut pixels);
            }
            return Some(insert_atlas_cache(key, [width, height], pixels));
        }
    }
    let bytes =
        Uint8Array::new_with_byte_offset_and_length(buffer, base, atlas_len as u32).to_vec();
    atlas_from_wire_bytes(coords, &bytes)
}

pub(crate) fn decode_symbol_layer_from_js(
    buffer: &ArrayBuffer,
    base_offset: u32,
    byte_len: u32,
) -> Option<SymbolLayerData> {
    let end = base_offset.checked_add(byte_len)?;
    if end > buffer.byte_length() || (byte_len as usize) < SYMBOL_HEADER_BYTES {
        return None;
    }
    let mut hdr_bytes = [0_u8; SYMBOL_HEADER_BYTES];
    Uint8Array::new_with_byte_offset_and_length(buffer, base_offset, SYMBOL_HEADER_BYTES as u32)
        .copy_to(&mut hdr_bytes);
    let hdr = parse_symbol_header(&hdr_bytes)?;
    let body = hdr
        .vert_count
        .checked_mul(std::mem::size_of::<ShaderSymbolVertex>())?
        .checked_add(hdr.idx_count.checked_mul(4)?)?
        .checked_add(hdr.layer_name_len)?
        .checked_add(hdr.style_id_len)?
        .checked_add(hdr.feat_len)?
        .checked_add(hdr.atlas_len)?;
    if SYMBOL_HEADER_BYTES.checked_add(body)? > byte_len as usize {
        return None;
    }

    let mut cur = base_offset + SYMBOL_HEADER_BYTES as u32;
    let mut vertices = vec![ShaderSymbolVertex::zeroed(); hdr.vert_count];
    copy_js_slice(buffer, &mut cur, bytemuck::cast_slice_mut(&mut vertices));

    let mut indices = vec![0_u32; hdr.idx_count];
    copy_js_slice(buffer, &mut cur, bytemuck::cast_slice_mut(&mut indices));

    let mut layer_buf = vec![0_u8; hdr.layer_name_len];
    copy_js_slice(buffer, &mut cur, &mut layer_buf);
    let source_layer = String::from_utf8(layer_buf).unwrap_or_default();

    let style_layer_id = if hdr.style_id_len == 0 {
        source_layer.clone()
    } else {
        let mut id_buf = vec![0_u8; hdr.style_id_len];
        copy_js_slice(buffer, &mut cur, &mut id_buf);
        String::from_utf8(id_buf).unwrap_or_default()
    };

    let features = if hdr.feat_len == 0 {
        Vec::new()
    } else {
        let mut feat_buf = vec![0_u8; hdr.feat_len];
        copy_js_slice(buffer, &mut cur, &mut feat_buf);
        maplibre::sdf::assets::wire::decode_symbol_features(&feat_buf).unwrap_or_else(|| {
            tracing::error!("invalid symbol features");
            Vec::new()
        })
    };

    let atlas = hdr
        .has_atlas
        .then(|| decode_atlas_from_js(hdr.coords, buffer, cur, hdr.atlas_len))
        .flatten();

    Some(SymbolLayerData {
        coords: hdr.coords,
        source_layer,
        style_layer_id,
        buffer: OverAlignedVertexBuffer::from_vecs(vertices, indices, hdr.usable_indices),
        features,
        atlas,
    })
}

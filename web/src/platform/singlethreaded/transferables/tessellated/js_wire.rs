//! Compact little-endian wire codec and direct JS `ArrayBuffer` reader for [`AvailableVectorLayerBucket`].
//!
//! Avoids copying multi-megabyte worker wire buffers into WASM linear memory as an intermediate
//! `Vec<u8>` before constructing typed `ShaderVertex` and index buffers.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(any(test, feature = "renderer"))]
use js_sys::{ArrayBuffer, Uint8Array};
#[cfg(any(test, feature = "renderer"))]
use maplibre::vector::feature_paint::{wire::decode_feature_paint, BucketFeatureEntry};
use maplibre::{
    benchmarking::tessellation::{IndexDataType, OverAlignedVertexBuffer},
    coords::{WorldTileCoords, ZoomLevel},
    render::ShaderVertex,
    vector::AvailableVectorLayerBucket,
};

use super::prepacked_wire::{
    assemble_bucket, build_decoded_buffer, encode_prepacked_layer_tessellated, format_for_magic,
    WireFormat, LAYER_TESSELLATED_RAW_MAGIC,
};

const HEADER_BYTES: usize = 60;

struct WireHeader {
    format: WireFormat,
    coords: WorldTileCoords,
    usable_indices: u32,
    layer_name_len: usize,
    style_layer_id_len: usize,
    vertex_count: usize,
    index_count: usize,
    feature_index_count: usize,
    feature_color_count: usize,
    feature_sort_key_count: usize,
    paint_bytes_len: usize,
    shared_geometry_key: Option<u64>,
}

fn read_u32_le(hdr: &[u8; HEADER_BYTES], offset: usize) -> u32 {
    let mut buf = [0_u8; 4];
    buf.copy_from_slice(&hdr[offset..offset + 4]);
    u32::from_le_bytes(buf)
}

fn read_u64_le(hdr: &[u8; HEADER_BYTES], offset: usize) -> u64 {
    let mut buf = [0_u8; 8];
    buf.copy_from_slice(&hdr[offset..offset + 8]);
    u64::from_le_bytes(buf)
}

fn read_i32_le(hdr: &[u8; HEADER_BYTES], offset: usize) -> i32 {
    let mut buf = [0_u8; 4];
    buf.copy_from_slice(&hdr[offset..offset + 4]);
    i32::from_le_bytes(buf)
}

fn parse_header(hdr: &[u8; HEADER_BYTES], total_len: usize) -> Option<WireHeader> {
    let format = format_for_magic(read_u32_le(hdr, 0))?;
    let x = read_i32_le(hdr, 4);
    let y = read_i32_le(hdr, 8);
    let z = u8::try_from(read_u32_le(hdr, 12)).ok()?;
    let usable_indices = read_u32_le(hdr, 16);
    let layer_name_len = read_u32_le(hdr, 20) as usize;
    let style_layer_id_len = read_u32_le(hdr, 24) as usize;
    let vertex_count = read_u32_le(hdr, 28) as usize;
    let index_count = read_u32_le(hdr, 32) as usize;
    let feature_index_count = read_u32_le(hdr, 36) as usize;
    let feature_color_count = read_u32_le(hdr, 40) as usize;
    let feature_sort_key_count = read_u32_le(hdr, 44) as usize;
    let paint_bytes_len = read_u32_le(hdr, 48) as usize;
    let raw_shared_key = read_u64_le(hdr, 52);

    let body_bytes = layer_name_len
        .checked_add(style_layer_id_len)?
        .checked_add(vertex_count.checked_mul(format.vertex_stride())?)?
        .checked_add(index_count.checked_mul(format.index_stride())?)?
        .checked_add(feature_index_count.checked_mul(4)?)?
        .checked_add(feature_color_count.checked_mul(16)?)?
        .checked_add(feature_sort_key_count.checked_mul(4)?)?
        .checked_add(paint_bytes_len)?;
    if HEADER_BYTES.checked_add(body_bytes)? > total_len {
        return None;
    }

    Some(WireHeader {
        format,
        coords: WorldTileCoords {
            x,
            y,
            z: ZoomLevel::new(z),
        },
        usable_indices,
        layer_name_len,
        style_layer_id_len,
        vertex_count,
        index_count,
        feature_index_count,
        feature_color_count,
        feature_sort_key_count,
        paint_bytes_len,
        shared_geometry_key: (raw_shared_key != 0).then_some(raw_shared_key),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn encode_layer_tessellated(
    coords: WorldTileCoords,
    buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    feature_indices: Vec<u32>,
    feature_colors: Vec<[f32; 4]>,
    feature_sort_keys: Vec<f32>,
    layer_name_str: &str,
    style_layer_id: &str,
    paint_bytes: Option<&[u8]>,
    style_type: &str,
    allow_prepack: bool,
) -> Vec<u8> {
    if (allow_prepack && !style_type.is_empty() && !buffer.buffer.vertices.is_empty())
        || buffer.prepacked_gpu().is_some()
    {
        return encode_prepacked_layer_tessellated(
            HEADER_BYTES,
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            layer_name_str,
            style_layer_id,
            paint_bytes,
            style_type,
        );
    }
    let shared_key = buffer.shared_geometry_key().unwrap_or(0);
    let usable_indices = buffer.usable_indices;
    let raw_vertices = buffer.buffer.vertices;
    let raw_indices = buffer.buffer.indices;
    let paint_slice = paint_bytes.unwrap_or_default();

    let total_len = HEADER_BYTES
        + layer_name_str.len()
        + style_layer_id.len()
        + raw_vertices.len() * std::mem::size_of::<ShaderVertex>()
        + raw_indices.len() * 4
        + feature_indices.len() * 4
        + feature_colors.len() * 16
        + feature_sort_keys.len() * 4
        + paint_slice.len();

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&LAYER_TESSELLATED_RAW_MAGIC.to_le_bytes());
    out.extend_from_slice(&coords.x.to_le_bytes());
    out.extend_from_slice(&coords.y.to_le_bytes());
    out.extend_from_slice(&u32::from(u8::from(coords.z)).to_le_bytes());
    out.extend_from_slice(&usable_indices.to_le_bytes());
    out.extend_from_slice(&(layer_name_str.len() as u32).to_le_bytes());
    out.extend_from_slice(&(style_layer_id.len() as u32).to_le_bytes());
    out.extend_from_slice(&(raw_vertices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(raw_indices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(feature_indices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(feature_colors.len() as u32).to_le_bytes());
    out.extend_from_slice(&(feature_sort_keys.len() as u32).to_le_bytes());
    out.extend_from_slice(&(paint_slice.len() as u32).to_le_bytes());
    out.extend_from_slice(&shared_key.to_le_bytes());

    out.extend_from_slice(layer_name_str.as_bytes());
    out.extend_from_slice(style_layer_id.as_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&raw_vertices));
    drop(raw_vertices);
    out.extend_from_slice(bytemuck::cast_slice(&raw_indices));
    drop(raw_indices);
    out.extend_from_slice(bytemuck::cast_slice(&feature_indices));
    drop(feature_indices);
    out.extend_from_slice(bytemuck::cast_slice(&feature_colors));
    drop(feature_colors);
    out.extend_from_slice(bytemuck::cast_slice(&feature_sort_keys));
    drop(feature_sort_keys);
    out.extend_from_slice(paint_slice);
    out
}

pub(super) fn peek_coords(bytes: &[u8]) -> Option<WorldTileCoords> {
    let hdr = bytes.first_chunk::<HEADER_BYTES>()?;
    Some(parse_header(hdr, bytes.len())?.coords)
}

pub(super) fn peek_usable_indices(bytes: &[u8]) -> Option<u32> {
    let hdr = bytes.first_chunk::<HEADER_BYTES>()?;
    Some(parse_header(hdr, bytes.len())?.usable_indices)
}

pub(super) fn peek_style_layer_id(bytes: &[u8]) -> Option<&str> {
    let hdr = bytes.first_chunk::<HEADER_BYTES>()?;
    let parsed = parse_header(hdr, bytes.len())?;
    let name_start = HEADER_BYTES;
    let id_start = name_start + parsed.layer_name_len;
    let id_end = id_start + parsed.style_layer_id_len;
    if parsed.style_layer_id_len > 0 {
        std::str::from_utf8(bytes.get(id_start..id_end)?).ok()
    } else {
        std::str::from_utf8(bytes.get(name_start..id_start)?).ok()
    }
}

pub(super) fn peek_paint_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let hdr = bytes.first_chunk::<HEADER_BYTES>()?;
    let parsed = parse_header(hdr, bytes.len())?;
    if parsed.paint_bytes_len == 0 {
        return None;
    }
    let paint_start = HEADER_BYTES
        + parsed.layer_name_len
        + parsed.style_layer_id_len
        + parsed.vertex_count * parsed.format.vertex_stride()
        + parsed.index_count * parsed.format.index_stride()
        + parsed.feature_index_count * 4
        + parsed.feature_color_count * 16
        + parsed.feature_sort_key_count * 4;
    bytes.get(paint_start..paint_start + parsed.paint_bytes_len)
}

pub(super) fn decode_bucket_from_bytes(bytes: &[u8]) -> Option<AvailableVectorLayerBucket> {
    let hdr = bytes.first_chunk::<HEADER_BYTES>()?;
    let h = parse_header(hdr, bytes.len())?;
    let mut cur = HEADER_BYTES;
    let take = |cur: &mut usize, len: usize| -> Option<&[u8]> {
        let end = cur.checked_add(len)?;
        let s = bytes.get(*cur..end)?;
        *cur = end;
        Some(s)
    };
    let source_layer = std::str::from_utf8(take(&mut cur, h.layer_name_len)?)
        .ok()?
        .to_owned();
    let style_raw = std::str::from_utf8(take(&mut cur, h.style_layer_id_len)?).ok()?;
    let style_layer_id = if style_raw.is_empty() {
        source_layer.clone()
    } else {
        style_raw.to_owned()
    };

    let buffer = build_decoded_buffer(
        h.format,
        h.usable_indices,
        h.shared_geometry_key,
        h.vertex_count,
        h.index_count,
        |v_dst, i_dst| {
            v_dst.copy_from_slice(take(&mut cur, v_dst.len())?);
            i_dst.copy_from_slice(take(&mut cur, i_dst.len())?);
            Some(())
        },
    )?;
    let mut feature_indices = vec![0_u32; h.feature_index_count];
    bytemuck::cast_slice_mut(&mut feature_indices)
        .copy_from_slice(take(&mut cur, h.feature_index_count * 4)?);
    let mut feature_colors = vec![[0.0_f32; 4]; h.feature_color_count];
    bytemuck::cast_slice_mut(&mut feature_colors)
        .copy_from_slice(take(&mut cur, h.feature_color_count * 16)?);
    let mut feature_sort_keys = vec![0.0_f32; h.feature_sort_key_count];
    bytemuck::cast_slice_mut(&mut feature_sort_keys)
        .copy_from_slice(take(&mut cur, h.feature_sort_key_count * 4)?);

    Some(assemble_bucket(
        h.coords,
        source_layer,
        style_layer_id,
        buffer,
        feature_indices,
        feature_colors,
        feature_sort_keys,
    ))
}

#[cfg(any(test, feature = "renderer"))]
pub(crate) fn decode_from_js_slice(
    buffer: &ArrayBuffer,
    base_offset: u32,
    byte_len: u32,
) -> Option<(
    Box<AvailableVectorLayerBucket>,
    Option<Vec<BucketFeatureEntry>>,
)> {
    let end = base_offset.checked_add(byte_len)?;
    if end > buffer.byte_length() || (byte_len as usize) < HEADER_BYTES {
        return None;
    }
    let mut hdr = [0_u8; HEADER_BYTES];
    Uint8Array::new_with_byte_offset_and_length(buffer, base_offset, HEADER_BYTES as u32)
        .copy_to(&mut hdr);
    let h = parse_header(&hdr, byte_len as usize)?;
    let mut cur = base_offset + HEADER_BYTES as u32;
    let copy_js = |cur: &mut u32, dst: &mut [u8]| {
        if !dst.is_empty() {
            let len = dst.len() as u32;
            Uint8Array::new_with_byte_offset_and_length(buffer, *cur, len).copy_to(dst);
            *cur += len;
        }
    };

    let mut name_buf = vec![0_u8; h.layer_name_len];
    copy_js(&mut cur, &mut name_buf);
    let source_layer = String::from_utf8(name_buf).ok()?;

    let mut id_buf = vec![0_u8; h.style_layer_id_len];
    copy_js(&mut cur, &mut id_buf);
    let style_raw = String::from_utf8(id_buf).ok()?;
    let style_layer_id = if style_raw.is_empty() {
        source_layer.clone()
    } else {
        style_raw
    };

    let buffer = build_decoded_buffer(
        h.format,
        h.usable_indices,
        h.shared_geometry_key,
        h.vertex_count,
        h.index_count,
        |v_dst, i_dst| {
            copy_js(&mut cur, v_dst);
            copy_js(&mut cur, i_dst);
            Some(())
        },
    )?;
    let mut feature_indices = vec![0_u32; h.feature_index_count];
    copy_js(&mut cur, bytemuck::cast_slice_mut(&mut feature_indices));
    let mut feature_colors = vec![[0.0_f32; 4]; h.feature_color_count];
    copy_js(&mut cur, bytemuck::cast_slice_mut(&mut feature_colors));
    let mut feature_sort_keys = vec![0.0_f32; h.feature_sort_key_count];
    copy_js(&mut cur, bytemuck::cast_slice_mut(&mut feature_sort_keys));

    let paint = if h.paint_bytes_len > 0 {
        let mut pb = vec![0_u8; h.paint_bytes_len];
        copy_js(&mut cur, &mut pb);
        decode_feature_paint(&pb)
    } else {
        None
    };

    Some((
        Box::new(assemble_bucket(
            h.coords,
            source_layer,
            style_layer_id,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
        )),
        paint,
    ))
}

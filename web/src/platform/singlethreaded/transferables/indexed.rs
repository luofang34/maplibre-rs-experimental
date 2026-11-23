//! Spatial and linear tile query indexes crossing the worker binary boundary.
use std::sync::Arc;

use maplibre::io::geometry_index::{
    wire::{WireGeometry, WireTileIndex},
    LazyMvtIndex,
};

use super::*;

const LAZY_MVT_MAGIC: [u8; 4] = *b"MLIX";
const LAZY_MVT_HEADER_LEN: usize = 20;
const WIRE_INDEX_MAGIC: [u8; 4] = *b"MLWI";
const WIRE_INDEX_HEADER_LEN: usize = 48;
const WIRE_GEOM_BYTES: usize = 40;

fn encode_lazy_mvt(
    coords: WorldTileCoords,
    source: Option<&str>,
    lazy: &LazyMvtIndex,
) -> Option<FlatBufferTransferable> {
    let source_bytes = source.map(str::as_bytes);
    let source_len_tag = match source_bytes {
        Some(bytes) => u32::try_from(bytes.len()).ok().filter(|&n| n < u32::MAX)?,
        None => u32::MAX,
    };
    let (mvt_bytes, compressed) = lazy.wire_bytes();
    let rescale = lazy.rescale();
    let rescale_len = if rescale.is_some() { 9 } else { 0 };
    let total =
        LAZY_MVT_HEADER_LEN + rescale_len + source_bytes.map_or(0, <[u8]>::len) + mvt_bytes.len();
    let mut data = Vec::with_capacity(total);
    data.extend_from_slice(&LAZY_MVT_MAGIC);
    data.extend_from_slice(&coords.x.to_le_bytes());
    data.extend_from_slice(&coords.y.to_le_bytes());
    data.push(u8::from(coords.z));
    data.push(u8::from(compressed));
    data.push(u8::from(rescale.is_some()));
    data.push(0);
    data.extend_from_slice(&source_len_tag.to_le_bytes());
    if let Some((from, _)) = rescale {
        data.extend_from_slice(&from.x.to_le_bytes());
        data.extend_from_slice(&from.y.to_le_bytes());
        data.push(u8::from(from.z));
    }
    if let Some(bytes) = source_bytes {
        data.extend_from_slice(bytes);
    }
    data.extend_from_slice(mvt_bytes);
    Some(FlatBufferTransferable::from_vec(
        WebMessageTag::LayerIndexed,
        data,
        0,
    ))
}

type ParsedLazyMvtHeader<'a> = (
    WorldTileCoords,
    bool,
    Option<(WorldTileCoords, WorldTileCoords)>,
    Option<&'a str>,
    &'a [u8],
);

fn parse_lazy_mvt_header(payload: &[u8]) -> Option<ParsedLazyMvtHeader<'_>> {
    if payload.len() < LAZY_MVT_HEADER_LEN || payload.get(..4) != Some(&LAZY_MVT_MAGIC) {
        return None;
    }
    let coords = read_coords_at(payload, 4)?;
    let compressed = *payload.get(13)? != 0;
    let has_rescale = *payload.get(14)? != 0;
    let source_len_tag = read_u32_le(payload, 16)?;
    let mut cursor = LAZY_MVT_HEADER_LEN;
    let rescale = if has_rescale {
        let from = read_coords_at(payload, cursor)?;
        cursor += 9;
        Some((from, coords))
    } else {
        None
    };
    let (source, mvt_offset) = if source_len_tag == u32::MAX {
        (None, cursor)
    } else {
        let len = usize::try_from(source_len_tag).ok()?;
        let end = cursor.checked_add(len)?;
        let src_bytes = payload.get(cursor..end)?;
        (Some(std::str::from_utf8(src_bytes).ok()?), end)
    };
    Some((
        coords,
        compressed,
        rescale,
        source,
        payload.get(mvt_offset..)?,
    ))
}

fn encode_wire_index(
    coords: WorldTileCoords,
    source: Option<&str>,
    wire: &WireTileIndex,
) -> FlatBufferTransferable {
    let source_bytes = source.map(str::as_bytes);
    let source_len = source_bytes.map_or(0, <[u8]>::len);
    let strings_bytes: usize = wire.strings.iter().map(|s| 4 + s.len()).sum();
    let total = WIRE_INDEX_HEADER_LEN
        + source_len
        + strings_bytes
        + wire.prop_blobs.len()
        + wire.prop_offsets.len() * 4
        + wire.geometries.len() * WIRE_GEOM_BYTES
        + wire.ring_ends.len() * 4
        + wire.xy.len() * 8;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&WIRE_INDEX_MAGIC);
    out.extend_from_slice(&coords.x.to_le_bytes());
    out.extend_from_slice(&coords.y.to_le_bytes());
    out.push(u8::from(coords.z));
    out.push(u8::from(wire.is_spatial));
    out.push(u8::from(source_bytes.is_some()));
    out.push(0);
    out.extend_from_slice(&(source_len as u32).to_le_bytes());
    out.extend_from_slice(&(wire.strings.len() as u32).to_le_bytes());
    out.extend_from_slice(&(wire.prop_blobs.len() as u32).to_le_bytes());
    out.extend_from_slice(&(wire.prop_offsets.len() as u32).to_le_bytes());
    out.extend_from_slice(&(wire.geometries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(wire.ring_ends.len() as u32).to_le_bytes());
    out.extend_from_slice(&(wire.xy.len() as u32).to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());

    if let Some(bytes) = source_bytes {
        out.extend_from_slice(bytes);
    }
    for s in &wire.strings {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    out.extend_from_slice(&wire.prop_blobs);
    for &off in &wire.prop_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    for g in &wire.geometries {
        out.extend_from_slice(&g.id.to_le_bytes());
        out.extend_from_slice(&g.source_layer_idx.to_le_bytes());
        out.extend_from_slice(&g.prop_group_idx.to_le_bytes());
        out.extend_from_slice(&g.feature_index.to_le_bytes());
        out.extend_from_slice(&g.ring_start.to_le_bytes());
        out.extend_from_slice(&g.ring_count.to_le_bytes());
        out.extend_from_slice(&g.coord_start.to_le_bytes());
        out.extend_from_slice(&g.coord_count.to_le_bytes());
        out.push(g.kind);
        out.push(u8::from(g.has_id));
        out.extend_from_slice(&[0, 0]);
    }
    for &end in &wire.ring_ends {
        out.extend_from_slice(&end.to_le_bytes());
    }
    for &val in &wire.xy {
        out.extend_from_slice(&val.to_le_bytes());
    }
    FlatBufferTransferable::from_vec(WebMessageTag::LayerIndexed, out, 0)
}

fn parse_wire_header(payload: &[u8]) -> Option<(WorldTileCoords, bool, Option<&str>, usize)> {
    if payload.len() < WIRE_INDEX_HEADER_LEN || payload.get(..4) != Some(&WIRE_INDEX_MAGIC) {
        return None;
    }
    let coords = read_coords_at(payload, 4)?;
    let is_spatial = *payload.get(13)? != 0;
    let has_source = *payload.get(14)? != 0;
    let source_len = read_u32_le(payload, 16)? as usize;
    let end = WIRE_INDEX_HEADER_LEN.checked_add(source_len)?;
    let source = if has_source {
        let slice = payload.get(WIRE_INDEX_HEADER_LEN..end)?;
        Some(std::str::from_utf8(slice).ok()?)
    } else {
        None
    };
    Some((coords, is_spatial, source, end))
}

fn decode_wire_index(payload: &[u8]) -> Option<TileIndex> {
    let (_, is_spatial, _, mut cursor) = parse_wire_header(payload)?;
    let strings_cnt = read_u32_le(payload, 20)? as usize;
    let prop_blobs_len = read_u32_le(payload, 24)? as usize;
    let prop_offsets_cnt = read_u32_le(payload, 28)? as usize;
    let geoms_cnt = read_u32_le(payload, 32)? as usize;
    let ring_ends_cnt = read_u32_le(payload, 36)? as usize;
    let xy_cnt = read_u32_le(payload, 40)? as usize;

    let mut strings = Vec::with_capacity(strings_cnt);
    for _ in 0..strings_cnt {
        let len = read_u32_le(payload, cursor)? as usize;
        cursor += 4;
        let end = cursor.checked_add(len)?;
        let s = std::str::from_utf8(payload.get(cursor..end)?)
            .ok()?
            .to_owned();
        strings.push(s);
        cursor = end;
    }
    let blobs_end = cursor.checked_add(prop_blobs_len)?;
    let prop_blobs = payload.get(cursor..blobs_end)?.to_vec();
    cursor = blobs_end;

    let mut prop_offsets = Vec::with_capacity(prop_offsets_cnt);
    for _ in 0..prop_offsets_cnt {
        prop_offsets.push(read_u32_le(payload, cursor)?);
        cursor += 4;
    }
    let mut geometries = Vec::with_capacity(geoms_cnt);
    for _ in 0..geoms_cnt {
        let g_slice = payload.get(cursor..cursor + WIRE_GEOM_BYTES)?;
        geometries.push(WireGeometry {
            id: read_u64_le(g_slice, 0)?,
            source_layer_idx: read_u32_le(g_slice, 8)?,
            prop_group_idx: read_u32_le(g_slice, 12)?,
            feature_index: read_u32_le(g_slice, 16)?,
            ring_start: read_u32_le(g_slice, 20)?,
            ring_count: read_u32_le(g_slice, 24)?,
            coord_start: read_u32_le(g_slice, 28)?,
            coord_count: read_u32_le(g_slice, 32)?,
            kind: *g_slice.get(36)?,
            has_id: *g_slice.get(37)? != 0,
        });
        cursor += WIRE_GEOM_BYTES;
    }
    let mut ring_ends = Vec::with_capacity(ring_ends_cnt);
    for _ in 0..ring_ends_cnt {
        ring_ends.push(read_u32_le(payload, cursor)?);
        cursor += 4;
    }
    let mut xy = Vec::with_capacity(xy_cnt);
    for _ in 0..xy_cnt {
        xy.push(read_f64_le(payload, cursor)?);
        cursor += 8;
    }
    Some(
        WireTileIndex {
            is_spatial,
            strings,
            prop_blobs,
            prop_offsets,
            geometries,
            ring_ends,
            xy,
        }
        .into_tile_index(),
    )
}

#[cfg(any(test, feature = "renderer"))]
fn decode_lazy_mvt_from_js(
    buffer: &js_sys::ArrayBuffer,
    base_offset: u32,
    byte_len: u32,
    hdr: &[u8; LAZY_MVT_HEADER_LEN],
) -> Option<FlatBufferTransferable> {
    let coords = read_coords_at(hdr, 4)?;
    let compressed = hdr[13] != 0;
    let has_rescale = hdr[14] != 0;
    let source_len_tag = read_u32_le(hdr, 16)?;
    let mut cursor = LAZY_MVT_HEADER_LEN;
    let rescale = if has_rescale {
        let from_end = cursor.checked_add(9)?;
        if from_end > byte_len as usize {
            return None;
        }
        let mut from_buf = [0_u8; 9];
        js_sys::Uint8Array::new_with_byte_offset_and_length(buffer, base_offset + cursor as u32, 9)
            .copy_to(&mut from_buf);
        cursor = from_end;
        Some((read_coords_at(&from_buf, 0)?, coords))
    } else {
        None
    };
    let (source, mvt_off) = if source_len_tag == u32::MAX {
        (None, cursor)
    } else {
        let len = usize::try_from(source_len_tag).ok()?;
        let off_end = cursor.checked_add(len)?;
        if off_end > byte_len as usize {
            return None;
        }
        let mut src_buf = vec![0_u8; len];
        if len > 0 {
            js_sys::Uint8Array::new_with_byte_offset_and_length(
                buffer,
                base_offset + cursor as u32,
                len as u32,
            )
            .copy_to(&mut src_buf);
        }
        (Some(String::from_utf8(src_buf).ok()?), off_end)
    };
    let mvt_len = (byte_len as usize).checked_sub(mvt_off)?;
    let mut mvt = vec![0_u8; mvt_len];
    if mvt_len > 0 {
        js_sys::Uint8Array::new_with_byte_offset_and_length(
            buffer,
            base_offset + mvt_off as u32,
            mvt_len as u32,
        )
        .copy_to(&mut mvt);
    }
    let index = TileIndex::LazyMvt(
        LazyMvtIndex::from_packed(Arc::from(mvt), compressed).with_rescale(rescale),
    );
    Some(FlatBufferTransferable::from_unpacked_index(
        coords, source, index,
    ))
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn decode_bytes_layer_indexed(bytes: &[u8]) -> Option<FlatBufferTransferable> {
    if let Some((coords, compressed, rescale, source, mvt_slice)) = parse_lazy_mvt_header(bytes) {
        let index = TileIndex::LazyMvt(
            LazyMvtIndex::from_packed(Arc::from(mvt_slice), compressed).with_rescale(rescale),
        );
        return Some(FlatBufferTransferable::from_unpacked_index(
            coords,
            source.map(str::to_owned),
            index,
        ));
    }
    if bytes.len() < WIRE_INDEX_HEADER_LEN {
        return None;
    }
    Some(FlatBufferTransferable::from_vec(
        WebMessageTag::LayerIndexed,
        bytes.to_vec(),
        0,
    ))
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn decode_js_layer_indexed(
    buffer: &js_sys::ArrayBuffer,
    base_offset: u32,
    byte_len: u32,
) -> Option<FlatBufferTransferable> {
    let end = base_offset.checked_add(byte_len)?;
    if end > buffer.byte_length() || (byte_len as usize) < LAZY_MVT_HEADER_LEN {
        return None;
    }
    let mut hdr = [0_u8; LAZY_MVT_HEADER_LEN];
    js_sys::Uint8Array::new_with_byte_offset_and_length(
        buffer,
        base_offset,
        LAZY_MVT_HEADER_LEN as u32,
    )
    .copy_to(&mut hdr);
    if hdr[..4] == LAZY_MVT_MAGIC {
        return decode_lazy_mvt_from_js(buffer, base_offset, byte_len, &hdr);
    }
    let bytes =
        js_sys::Uint8Array::new_with_byte_offset_and_length(buffer, base_offset, byte_len).to_vec();
    Some(FlatBufferTransferable::from_vec(
        WebMessageTag::LayerIndexed,
        bytes,
        0,
    ))
}

impl LayerIndexed for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerIndexed
    }

    fn build_from(coords: WorldTileCoords, source: Option<String>, index: TileIndex) -> Self {
        if let TileIndex::LazyMvt(ref lazy) = index {
            if let Some(fast) = encode_lazy_mvt(coords, source.as_deref(), lazy) {
                return fast;
            }
        }
        let wire = WireTileIndex::from_tile_index(&index);
        encode_wire_index(coords, source.as_deref(), &wire)
    }

    fn coords(&self) -> WorldTileCoords {
        if let Some((coords, _, _)) = &self.unpacked_index {
            return *coords;
        }
        if let Some((coords, _, _, _, _)) = parse_lazy_mvt_header(self.data()) {
            return coords;
        }
        parse_wire_header(self.data()).map_or_else(WorldTileCoords::default, |h| h.0)
    }

    fn source(&self) -> Option<String> {
        if let Some((_, source, _)) = &self.unpacked_index {
            return source.clone();
        }
        if let Some((_, _, _, source, _)) = parse_lazy_mvt_header(self.data()) {
            return source.map(str::to_owned);
        }
        parse_wire_header(self.data()).and_then(|h| h.2.map(str::to_owned))
    }

    fn to_tile_index(mut self) -> TileIndex {
        if let Some((_, _, index)) = self.unpacked_index.take() {
            return index;
        }
        if let Some((_, compressed, rescale, _, mvt_bytes)) = parse_lazy_mvt_header(self.data()) {
            return TileIndex::LazyMvt(
                LazyMvtIndex::from_packed(Arc::from(mvt_bytes), compressed).with_rescale(rescale),
            );
        }
        decode_wire_index(self.data()).unwrap_or_else(|| WireTileIndex::default().into_tile_index())
    }
}

#[cfg(test)]
mod tests;

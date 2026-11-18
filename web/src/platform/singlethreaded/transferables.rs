//! Compact little-endian worker wire transferables without FlatBuffers overhead.

use std::fmt::{Debug, Formatter};

#[cfg(test)]
use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::{
    benchmarking::tessellation::{IndexDataType, OverAlignedVertexBuffer},
    coords::{WorldTileCoords, ZoomLevel},
    io::{
        apc::{IntoMessage, Message, MessageTag},
        geometry_index::TileIndex,
    },
    raster::{
        AvailableRasterLayerData, LayerRaster, LayerRasterMissing, MissingRasterLayerData,
        RasterSourceId, RasterTransferables, RgbaImage,
    },
    render::shaders::ShaderSymbolVertex,
    sdf::{Feature, SymbolLayerData},
    terrain::transferables::{DemTransferables, LayerDem, LayerDemMissing},
    tile::Layer,
    vector::{
        LayerIndexed, LayerMissing, MissingVectorLayerBucket, SymbolLayerTessellated,
        TileTessellated, VectorTransferables,
    },
};

use crate::platform::singlethreaded::apc::WebMessageTag;

/// Owned byte buffer carrying a single worker-to-main transferable payload.
pub struct FlatBufferTransferable {
    tag: WebMessageTag,
    data: Vec<u8>,
    start: usize,
    pub(crate) unpacked_vector: Option<(
        Box<maplibre::vector::AvailableVectorLayerBucket>,
        Option<Vec<maplibre::vector::feature_paint::BucketFeatureEntry>>,
    )>,
}

impl FlatBufferTransferable {
    #[cfg(test)]
    pub fn from_array_buffer(tag: WebMessageTag, buffer: ArrayBuffer) -> Self {
        let buffer = Uint8Array::new(&buffer);
        Self {
            tag,
            data: buffer.to_vec(),
            start: 0,
            unpacked_vector: None,
        }
    }

    pub fn from_vec(tag: WebMessageTag, data: Vec<u8>, start: usize) -> Self {
        Self {
            tag,
            data,
            start,
            unpacked_vector: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_layer_tessellated(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        let (bucket, paint) = tessellated::js_wire::decode_from_js_slice(buffer, offset, len)?;
        Some(Self {
            tag: WebMessageTag::LayerTessellated,
            data: Vec::new(),
            start: 0,
            unpacked_vector: Some((bucket, paint)),
        })
    }

    pub fn data(&self) -> &[u8] {
        &self.data[self.start..]
    }

    pub(super) fn into_tail_vec(mut self, payload_offset: usize) -> Vec<u8> {
        let total = self.start.saturating_add(payload_offset);
        if total > 0 && total <= self.data.len() {
            self.data.drain(..total);
        } else if total > self.data.len() {
            self.data.clear();
        }
        self.data
    }
}

#[inline]
pub(super) fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_le_bytes(arr))
}

#[inline]
pub(super) fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice = bytes.get(offset..offset + 4)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(i32::from_le_bytes(arr))
}

#[inline]
pub(super) fn read_u64_le(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset + 8)?;
    let arr: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(arr))
}

#[inline]
pub(super) fn read_f64_le(bytes: &[u8], offset: usize) -> Option<f64> {
    read_u64_le(bytes, offset).map(f64::from_bits)
}

#[inline]
pub(super) fn read_coords_at(bytes: &[u8], offset: usize) -> Option<WorldTileCoords> {
    let x = read_i32_le(bytes, offset)?;
    let y = read_i32_le(bytes, offset + 4)?;
    let z = *bytes.get(offset + 8)?;
    Some(WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    })
}

pub(super) fn encode_named_coords(
    tag: WebMessageTag,
    coords: WorldTileCoords,
    name: Option<&str>,
) -> FlatBufferTransferable {
    let name_bytes = name.map(str::as_bytes);
    let name_len = name_bytes.map_or(0, <[u8]>::len);
    let mut data = Vec::with_capacity(16 + name_len);
    data.extend_from_slice(&coords.x.to_le_bytes());
    data.extend_from_slice(&coords.y.to_le_bytes());
    data.push(u8::from(coords.z));
    data.push(u8::from(name_bytes.is_some()));
    data.extend_from_slice(&[0, 0]);
    data.extend_from_slice(&(name_len as u32).to_le_bytes());
    if let Some(bytes) = name_bytes {
        data.extend_from_slice(bytes);
    }
    FlatBufferTransferable::from_vec(tag, data, 0)
}

pub(super) fn decode_named_coords(bytes: &[u8]) -> (WorldTileCoords, Option<&str>) {
    let Some(coords) = read_coords_at(bytes, 0) else {
        return (WorldTileCoords::default(), None);
    };
    let has_name = bytes.get(9).is_some_and(|&b| b != 0);
    if !has_name {
        return (coords, None);
    }
    let name = read_u32_le(bytes, 12)
        .and_then(|len| bytes.get(16..16 + len as usize))
        .and_then(|slice| std::str::from_utf8(slice).ok());
    (coords, name)
}

impl TileTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::TileTessellated
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        tile_completion(coords, false, false)
    }

    fn build_partial(coords: WorldTileCoords) -> Self {
        tile_completion(coords, true, false)
    }

    fn build_failed(coords: WorldTileCoords, pending_symbols: bool) -> Self {
        tile_completion(coords, pending_symbols, true)
    }

    fn failed(&self) -> bool {
        self.data().get(10).is_none_or(|&b| b != 0)
    }

    fn pending_symbols(&self) -> bool {
        self.data().get(9).is_some_and(|&b| b != 0)
    }

    fn coords(&self) -> WorldTileCoords {
        self.data()
            .get(..11)
            .and_then(|bytes| read_coords_at(bytes, 0))
            .unwrap_or_default()
    }
}

impl LayerMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerMissing
    }

    fn build_from(coords: WorldTileCoords, layer_name: String) -> Self {
        encode_named_coords(WebMessageTag::LayerMissing, coords, Some(&layer_name))
    }

    fn coords(&self) -> WorldTileCoords {
        decode_named_coords(self.data()).0
    }

    fn layer_name(&self) -> &str {
        decode_named_coords(self.data()).1.unwrap_or_default()
    }

    fn to_bucket(self) -> MissingVectorLayerBucket {
        let (coords, name) = decode_named_coords(self.data());
        MissingVectorLayerBucket {
            source_layer: name.unwrap_or_default().to_owned(),
            coords,
        }
    }
}

impl Debug for FlatBufferTransferable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "FlatBufferTransferable<{:?}>(??)", self.tag)
    }
}

impl IntoMessage for FlatBufferTransferable {
    fn into(self) -> Message {
        Message::new(self.tag.to_static(), Box::new(self))
    }
}

mod indexed;
mod tessellated;

mod raster;

mod symbol;

#[derive(Copy, Clone)]
pub struct FlatTransferables;

impl VectorTransferables for FlatTransferables {
    type TileTessellated = FlatBufferTransferable;
    type LayerMissing = FlatBufferTransferable;
    type LayerTessellated = FlatBufferTransferable;
    type SymbolLayerTessellated = FlatBufferTransferable;
    type LayerIndexed = FlatBufferTransferable;
}

impl RasterTransferables for FlatTransferables {
    type LayerRaster = FlatBufferTransferable;
    type LayerRasterMissing = FlatBufferTransferable;
}

mod terrain;

mod completion;
use self::completion::tile_completion;

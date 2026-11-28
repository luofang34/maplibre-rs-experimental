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
    vector::{LayerIndexed, SymbolLayerTessellated, VectorTransferables},
};

use super::tag::WebMessageTag;

pub(crate) type UnpackedDem = (
    WorldTileCoords,
    Result<(std::sync::Arc<maplibre::terrain::DemTile>, f64), RgbaImage>,
);

#[cfg(any(test, feature = "renderer"))]
pub(crate) enum UnpackedTiny {
    Completion(WorldTileCoords, bool, bool),
    Named(WorldTileCoords, Option<String>),
}

/// Owned byte buffer carrying a single worker-to-main transferable payload.
pub struct FlatBufferTransferable {
    tag: WebMessageTag,
    data: Vec<u8>,
    start: usize,
    #[cfg(any(test, feature = "renderer"))]
    pub(super) unpacked_tiny: Option<UnpackedTiny>,
    pub(crate) unpacked_vector: Option<(
        Box<maplibre::vector::AvailableVectorLayerBucket>,
        Option<Vec<maplibre::vector::feature_paint::BucketFeatureEntry>>,
    )>,
    pub(crate) unpacked_symbol: Option<Box<maplibre::sdf::SymbolLayerData>>,
    pub(crate) unpacked_index: Option<(WorldTileCoords, Option<String>, TileIndex)>,
    pub(crate) unpacked_raster:
        Option<Result<AvailableRasterLayerData, Box<maplibre::raster::SharedRasterDem>>>,
    pub(crate) unpacked_dem: Option<UnpackedDem>,
}

impl FlatBufferTransferable {
    #[cfg(test)]
    pub fn from_array_buffer(tag: WebMessageTag, buffer: ArrayBuffer) -> Self {
        let buffer = Uint8Array::new(&buffer);
        Self {
            tag,
            data: buffer.to_vec(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    pub fn from_vec(tag: WebMessageTag, data: Vec<u8>, start: usize) -> Self {
        Self {
            tag,
            data,
            start,
            #[cfg(any(test, feature = "renderer"))]
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    fn from_unpacked_tiny(tag: WebMessageTag, tiny: UnpackedTiny) -> Self {
        Self {
            tag,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: Some(tiny),
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_tile_tessellated(bytes: &[u8]) -> Option<Self> {
        let coords = read_coords_at(bytes, 0)?;
        let pending = bytes.get(9).is_some_and(|&b| b != 0);
        let failed = bytes.get(10).is_some_and(|&b| b != 0);
        Some(Self::from_unpacked_tiny(
            WebMessageTag::TileTessellated,
            UnpackedTiny::Completion(coords, pending, failed),
        ))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_missing(bytes: &[u8]) -> Option<Self> {
        let (coords, name) = parse_named_coords_strict(bytes)?;
        Some(Self::from_unpacked_tiny(
            WebMessageTag::LayerMissing,
            UnpackedTiny::Named(coords, name.map(str::to_owned)),
        ))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_raster_missing(bytes: &[u8]) -> Option<Self> {
        let (coords, name) = parse_named_coords_strict(bytes)?;
        Some(Self::from_unpacked_tiny(
            WebMessageTag::LayerRasterMissing,
            UnpackedTiny::Named(coords, name.map(str::to_owned)),
        ))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_dem_missing(bytes: &[u8]) -> Option<Self> {
        let coords = read_coords_at(bytes, 0)?;
        Some(Self::from_unpacked_tiny(
            WebMessageTag::LayerDemMissing,
            UnpackedTiny::Named(coords, None),
        ))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_unpacked_index(
        coords: WorldTileCoords,
        source: Option<String>,
        index: TileIndex,
    ) -> Self {
        Self {
            tag: WebMessageTag::LayerIndexed,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: Some((coords, source, index)),
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_unpacked_raster(
        res: Result<AvailableRasterLayerData, Box<maplibre::raster::SharedRasterDem>>,
    ) -> Self {
        Self {
            tag: WebMessageTag::LayerRaster,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: Some(res),
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_unpacked_dem(
        coords: WorldTileCoords,
        outcome: Result<(std::sync::Arc<maplibre::terrain::DemTile>, f64), RgbaImage>,
    ) -> Self {
        Self {
            tag: WebMessageTag::LayerDem,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: Some((coords, outcome)),
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_layer_tessellated(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        let (bucket, paint) = tessellated::js_wire::decode_from_js_slice(buffer, offset, len)?;
        Some(Self::from_unpacked_vector(bucket, paint))
    }

    #[cfg(any(test, feature = "renderer"))]
    fn from_unpacked_vector(
        bucket: Box<maplibre::vector::AvailableVectorLayerBucket>,
        paint: Option<Vec<maplibre::vector::feature_paint::BucketFeatureEntry>>,
    ) -> Self {
        Self {
            tag: WebMessageTag::LayerTessellated,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: Some((bucket, paint)),
            unpacked_symbol: None,
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_tessellated(bytes: &[u8]) -> Option<Self> {
        let (bucket, paint) = tessellated::js_wire::decode_from_bytes(bytes)?;
        Some(Self::from_unpacked_vector(bucket, paint))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_symbol_layer(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        let bucket = symbol::js_wire::decode_symbol_layer_from_js(buffer, offset, len)?;
        Some(Self::from_unpacked_symbol(bucket))
    }

    #[cfg(any(test, feature = "renderer"))]
    fn from_unpacked_symbol(bucket: maplibre::sdf::SymbolLayerData) -> Self {
        Self {
            tag: WebMessageTag::SymbolLayerTessellated,
            data: Vec::new(),
            start: 0,
            unpacked_tiny: None,
            unpacked_vector: None,
            unpacked_symbol: Some(Box::new(bucket)),
            unpacked_index: None,
            unpacked_raster: None,
            unpacked_dem: None,
        }
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_symbol_layer(bytes: &[u8]) -> Option<Self> {
        let bucket = symbol::decode_symbol_bucket_from_bytes(bytes)?;
        Some(Self::from_unpacked_symbol(bucket))
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_layer_indexed(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        Self::decode_js_layer_indexed(buffer, offset, len)
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_indexed(bytes: &[u8]) -> Option<Self> {
        Self::decode_bytes_layer_indexed(bytes)
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_layer_raster(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        Self::decode_js_layer_raster(buffer, offset, len)
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_raster(bytes: &[u8]) -> Option<Self> {
        Self::decode_bytes_layer_raster(bytes)
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_js_layer_dem(
        buffer: &js_sys::ArrayBuffer,
        offset: u32,
        len: u32,
    ) -> Option<Self> {
        Self::decode_js_layer_dem(buffer, offset, len)
    }

    #[cfg(any(test, feature = "renderer"))]
    pub(crate) fn from_bytes_layer_dem(bytes: &[u8]) -> Option<Self> {
        Self::decode_bytes_layer_dem(bytes)
    }

    pub fn data(&self) -> &[u8] {
        self.data.get(self.start..).unwrap_or_default()
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
#[cfg(any(test, feature = "renderer"))]
pub(crate) use self::symbol::clear_atlas_cache as clear_symbol_atlas_cache;

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
use self::completion::decode_named_coords;
#[cfg(any(test, feature = "renderer"))]
use self::completion::parse_named_coords_strict;

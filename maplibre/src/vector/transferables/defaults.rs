//! Default owned Rust implementations of [`super::VectorTransferables`].

use std::fmt::{Debug, Formatter};

use geozero::mvt::tile::Layer;

use super::{
    LayerIndexed, LayerMissing, LayerTessellated, SymbolLayerTessellated, TileTessellated,
    VectorMessageTag, VectorTransferables,
};
use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{IntoMessage, Message, MessageTag},
        geometry_index::TileIndex,
    },
    render::{shaders::ShaderSymbolVertex, ShaderVertex},
    sdf::{Feature, SymbolLayerData},
    vector::{
        feature_paint::BucketFeatureEntry,
        tessellation::{IndexDataType, OverAlignedVertexBuffer},
        AvailableVectorLayerBucket, MissingVectorLayerBucket,
    },
};

/// Owned completion marker for a tile processing batch.
pub struct DefaultTileTessellated {
    coords: WorldTileCoords,
    pending_symbols: bool,
    failed: bool,
}

impl Debug for DefaultTileTessellated {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultTileTessellated({})", self.coords)
    }
}

impl IntoMessage for DefaultTileTessellated {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl TileTessellated for DefaultTileTessellated {
    fn message_tag() -> &'static dyn MessageTag {
        &VectorMessageTag::TileTessellated
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        Self {
            coords,
            pending_symbols: false,
            failed: false,
        }
    }

    fn build_failed(coords: WorldTileCoords, pending_symbols: bool) -> Self {
        Self {
            coords,
            pending_symbols,
            failed: true,
        }
    }

    fn failed(&self) -> bool {
        self.failed
    }

    fn build_partial(coords: WorldTileCoords) -> Self {
        Self {
            coords,
            pending_symbols: true,
            failed: false,
        }
    }

    fn pending_symbols(&self) -> bool {
        self.pending_symbols
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }
}

/// Owned record of a source layer absent from a tile.
pub struct DefaultLayerMissing {
    /// Tile-grid coordinates of the missing layer.
    pub coords: WorldTileCoords,
    /// Missing source-layer name, rather than a style-layer ID.
    pub layer_name: String,
}

impl Debug for DefaultLayerMissing {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultLayerMissing({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerMissing {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerMissing for DefaultLayerMissing {
    fn message_tag() -> &'static dyn MessageTag {
        &VectorMessageTag::LayerMissing
    }

    fn build_from(coords: WorldTileCoords, layer_name: String) -> Self {
        Self { coords, layer_name }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn layer_name(&self) -> &str {
        &self.layer_name
    }

    fn to_bucket(self) -> MissingVectorLayerBucket {
        MissingVectorLayerBucket {
            coords: self.coords,
            source_layer: self.layer_name,
        }
    }
}

#[derive(Clone)]
/// Owned tessellated geometry and paint metadata for one source/style layer pair.
pub struct DefaultLayerTessellated {
    /// Tile-grid coordinates whose local space contains the geometry.
    pub coords: WorldTileCoords,
    /// Vertex and index storage, including alignment padding and a usable index count.
    pub buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    /// Vertex counts per feature in buffer order, excluding alignment padding.
    pub feature_indices: Vec<u32>,
    /// Normalized RGBA paint colors in feature order; missing entries use the renderer's fallback.
    pub feature_colors: Vec<[f32; 4]>,
    /// The sort key of each feature, in feature order; empty without a sort key.
    pub feature_sort_keys: Vec<f32>,
    /// Decoded source layer; bucket conversion retains its name.
    pub layer_data: Layer,
    /// Style entry whose paint and layout produced this bucket.
    pub style_layer_id: String,
    /// Tile magnification zoom used when tessellating this layer.
    pub overscaled_zoom: u8,
    /// Retained per-entry feature attributes for in-place paint and `feature-state` refresh.
    pub feature_paint: Option<Vec<BucketFeatureEntry>>,
}

impl Debug for DefaultLayerTessellated {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultLayerTesselated({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerTessellated {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerTessellated for DefaultLayerTessellated {
    fn message_tag() -> &'static dyn MessageTag {
        &VectorMessageTag::LayerTessellated
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        Self {
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            layer_data,
            style_layer_id,
            overscaled_zoom: u8::from(coords.z),
            feature_paint: None,
        }
    }

    fn build_from_with_paint_typed(
        coords: WorldTileCoords,
        mut buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
        entries: Vec<BucketFeatureEntry>,
        style_type: &str,
        allow_prepack: bool,
    ) -> Self {
        if allow_prepack && buffer.prepacked_gpu().is_none() && !buffer.buffer.vertices.is_empty() {
            let logical_count = buffer.buffer.vertices.len();
            let mut packed = Vec::new();
            let compact_extrusion = feature_colors.is_empty()
                && entries.is_empty()
                && buffer.shared_geometry_key().is_none()
                && <ShaderVertex as crate::render::shaders::PoolVertex>::try_encode_compact_extrusion(
                    style_type,
                    &buffer.buffer.vertices,
                    &mut packed,
                )
                .is_some();
            if !compact_extrusion {
                let _ = <ShaderVertex as crate::render::shaders::PoolVertex>::encode_vertices(
                    style_type,
                    &buffer.buffer.vertices,
                    &mut packed,
                );
            }
            let shared_key = buffer.shared_geometry_key();
            let usable = buffer.usable_indices;
            let raw_indices = std::mem::take(&mut buffer.buffer.indices);
            let usable_len = (usable as usize).min(raw_indices.len());
            let logical_vertex_count = u32::try_from(logical_count).unwrap_or(u32::MAX);
            let can_u16 = logical_count <= 65_536
                && raw_indices[..usable_len]
                    .iter()
                    .all(|&idx| idx <= u32::from(u16::MAX));
            buffer = if can_u16 {
                let padded = usable_len.next_multiple_of(2);
                let mut u16s = Vec::with_capacity(padded);
                u16s.extend(raw_indices[..usable_len].iter().map(|&idx| idx as u16));
                if u16s.len() < padded {
                    u16s.push(0);
                }
                OverAlignedVertexBuffer::from_vecs(Vec::new(), Vec::new(), usable)
                    .with_prepacked_gpu(crate::vector::tessellation::PrepackedGpuGeometry {
                        vertices: packed,
                        indices_u16: Some(u16s),
                        compact_extrusion,
                        logical_vertex_count,
                    })
                    .with_shared_geometry_key(shared_key)
            } else {
                OverAlignedVertexBuffer::from_vecs(Vec::new(), raw_indices, usable)
                    .with_prepacked_gpu(crate::vector::tessellation::PrepackedGpuGeometry {
                        vertices: packed,
                        indices_u16: None,
                        compact_extrusion,
                        logical_vertex_count,
                    })
                    .with_shared_geometry_key(shared_key)
            };
        }
        Self::build_from_with_paint(
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            layer_data,
            style_layer_id,
            entries,
        )
    }

    fn with_overscaled_zoom(mut self, overscaled_zoom: u8) -> Self {
        self.overscaled_zoom = overscaled_zoom;
        self
    }

    fn overscaled_zoom(&self) -> u8 {
        self.overscaled_zoom
    }

    fn with_feature_paint(mut self, entries: Vec<BucketFeatureEntry>) -> Self {
        self.feature_paint = (!entries.is_empty()).then_some(entries);
        self
    }

    fn take_feature_paint(&mut self) -> Option<Vec<BucketFeatureEntry>> {
        self.feature_paint.take()
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn is_empty(&self) -> bool {
        self.buffer.usable_indices == 0
    }

    fn style_layer_id(&self) -> &str {
        &self.style_layer_id
    }

    fn to_bucket(self) -> AvailableVectorLayerBucket {
        AvailableVectorLayerBucket {
            coords: self.coords,
            source_layer: self.layer_data.name,
            style_layer_id: self.style_layer_id,
            buffer: self.buffer,
            feature_indices: self.feature_indices,
            feature_colors: self.feature_colors,
            feature_sort_keys: self.feature_sort_keys,
        }
    }
}

/// Owned symbol geometry and placement metadata with shared atlas ownership.
pub struct DefaultSymbolLayerTessellated {
    /// Glyph and sprite texels shared with other messages using the same atlas.
    pub atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
    /// Tile-grid coordinates whose local space contains the symbol anchors.
    pub coords: WorldTileCoords,
    /// Padded symbol vertex/index storage and usable draw index count.
    pub buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
    /// Placement metadata and geometry ranges for the symbols in this buffer.
    pub features: Vec<Feature>,
    /// Decoded source layer; bucket conversion retains its name.
    pub layer_data: Layer,
    /// Style entry whose symbol layout produced the geometry.
    pub style_layer_id: String,
}

impl Debug for DefaultSymbolLayerTessellated {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultSymbolLayerTessellated({})", self.coords)
    }
}

impl IntoMessage for DefaultSymbolLayerTessellated {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl SymbolLayerTessellated for DefaultSymbolLayerTessellated {
    fn message_tag() -> &'static dyn MessageTag {
        &VectorMessageTag::SymbolLayerTessellated
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        Self {
            atlas,
            coords,
            buffer,
            features,
            layer_data,
            style_layer_id,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn is_empty(&self) -> bool {
        self.buffer.usable_indices == 0
    }

    fn to_bucket(self) -> SymbolLayerData {
        SymbolLayerData {
            atlas: self.atlas,
            coords: self.coords,
            source_layer: self.layer_data.name,
            style_layer_id: self.style_layer_id,
            buffer: self.buffer,
            features: self.features,
        }
    }
}

/// Owned spatial index and the tile coordinates to which it belongs.
pub struct DefaultLayerIndexed {
    coords: WorldTileCoords,
    source: Option<String>,
    index: TileIndex,
}

impl Debug for DefaultLayerIndexed {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultLayerIndexed({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerIndexed {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerIndexed for DefaultLayerIndexed {
    fn message_tag() -> &'static dyn MessageTag {
        &VectorMessageTag::LayerIndexed
    }

    fn build_from(coords: WorldTileCoords, source: Option<String>, index: TileIndex) -> Self {
        Self {
            coords,
            source,
            index,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn source(&self) -> Option<String> {
        self.source.clone()
    }

    fn to_tile_index(self) -> TileIndex {
        self.index
    }
}

#[derive(Copy, Clone)]
/// Uses owned Rust payloads without serializing them for worker transport.
pub struct DefaultVectorTransferables;

impl VectorTransferables for DefaultVectorTransferables {
    type TileTessellated = DefaultTileTessellated;
    type LayerMissing = DefaultLayerMissing;
    type LayerTessellated = DefaultLayerTessellated;
    type SymbolLayerTessellated = DefaultSymbolLayerTessellated;
    type LayerIndexed = DefaultLayerIndexed;
}

#[cfg(test)]
mod tests;

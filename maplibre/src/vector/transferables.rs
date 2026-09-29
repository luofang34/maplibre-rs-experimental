//! Worker message contracts for vector geometry, symbols and tile query indices.

#![deny(missing_docs)]

use std::fmt::{Debug, Formatter};

use geozero::mvt::tile::Layer;

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{IntoMessage, Message, MessageTag},
        geometry_index::TileIndex,
    },
    render::{shaders::ShaderSymbolVertex, ShaderVertex},
    sdf::{Feature, SymbolLayerData},
    vector::{
        tessellation::{IndexDataType, OverAlignedVertexBuffer},
        AvailableVectorLayerBucket, MissingVectorLayerBucket,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
/// Tags used by the owned Rust vector message implementations.
pub enum VectorMessageTag {
    /// A tile processing batch finished, with or without pending symbol assets.
    TileTessellated = 1,
    /// A requested source layer is absent from the tile.
    LayerMissing = 2,
    /// Fill, line or circle geometry is ready for upload.
    LayerTessellated = 3,
    /// Symbol geometry and its placement metadata are ready.
    SymbolLayerTessellated = 4,
    /// Tile geometry is indexed for spatial queries.
    LayerIndexed = 10,
}

impl MessageTag for VectorMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// Tile completion marker; partial completion keeps pending symbol work in the request budget.
pub trait TileTessellated: IntoMessage + Debug + Send {
    /// Identifies this backend's completion payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Marks a processing batch complete with no outstanding symbol work.
    fn build_from(coords: WorldTileCoords) -> Self
    where
        Self: Sized;

    /// Reports unavailable source data, retaining parent coverage.
    /// Keep `pending_symbols` true until the worker sends its final completion.
    fn build_failed(coords: WorldTileCoords, pending_symbols: bool) -> Self
    where
        Self: Sized;
    /// Whether at least one requested source could not be fetched or decoded.
    fn failed(&self) -> bool;

    /// Marks base geometry ready while symbol assets are still pending.
    fn build_partial(coords: WorldTileCoords) -> Self
    where
        Self: Sized;
    /// Whether the tile still needs its symbol-processing completion.
    fn pending_symbols(&self) -> bool;
    /// Tile-grid coordinates of the completed processing batch.
    fn coords(&self) -> WorldTileCoords;
}

/// A requested source layer cannot produce a vector bucket for a tile.
pub trait LayerMissing: IntoMessage + Debug + Send {
    /// Identifies this backend's missing-layer payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Records a missing source-layer name at the supplied tile coordinates.
    fn build_from(coords: WorldTileCoords, layer_name: String) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates of the missing layer.
    fn coords(&self) -> WorldTileCoords;

    /// Source-layer name, rather than a style-layer ID.
    fn layer_name(&self) -> &str;

    /// Consumes the message into the tile's missing-layer record.
    fn to_bucket(self) -> MissingVectorLayerBucket;
}

/// Tessellated vector geometry for one style layer, ready for renderer upload.
pub trait LayerTessellated: IntoMessage + Debug + Send {
    /// Identifies this backend's vector-geometry payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Takes ownership of padded geometry, per-feature vertex counts and RGBA colors.
    /// Metadata follows geometry feature order; `layer_data` names the source layer, while
    /// `style_layer_id` identifies the style entry whose paint produced this bucket.
    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates whose local space contains the geometry.
    fn coords(&self) -> WorldTileCoords;

    /// Whether the geometry contains no usable draw indices, excluding alignment padding.
    fn is_empty(&self) -> bool;

    /// Style entry to use when uploading and drawing the bucket.
    fn style_layer_id(&self) -> &str;

    /// Consumes the transfer representation into renderer-owned geometry and metadata.
    fn to_bucket(self) -> AvailableVectorLayerBucket;
}

/// Text or icon geometry with placement metadata and an optional shared glyph/sprite atlas.
pub trait SymbolLayerTessellated: IntoMessage + Debug + Send {
    /// Identifies this backend's symbol payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Takes ownership of symbol geometry and feature ranges for one source/style layer pair.
    /// Atlas ownership is shared so its texels remain available when the renderer uploads them.
    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates whose local space contains the symbol anchors.
    fn coords(&self) -> WorldTileCoords;

    /// Whether there are no usable symbol draw indices, excluding alignment padding.
    fn is_empty(&self) -> bool;

    /// Consumes the message into the renderer's symbol layer data.
    fn to_bucket(self) -> SymbolLayerData;
}

/// Spatial index produced from a tile's decoded vector geometry.
pub trait LayerIndexed: IntoMessage + Debug + Send {
    /// Identifies this backend's spatial-index payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Associates an owned spatial index with its tile-grid coordinates.
    fn build_from(coords: WorldTileCoords, index: TileIndex) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates associated with the index.
    fn coords(&self) -> WorldTileCoords;

    /// Consumes the transfer representation and returns its spatial index.
    fn to_tile_index(self) -> TileIndex;
}

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
    /// Decoded source layer; bucket conversion retains its name.
    pub layer_data: Layer, // FIXME (perf): Introduce a better structure for this
    /// Style entry whose paint and layout produced this bucket.
    pub style_layer_id: String,
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
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        Self {
            coords,
            buffer,
            feature_indices,
            feature_colors,
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
    pub layer_data: Layer, // FIXME (perf): Introduce a better structure for this
    /// Style entry whose symbol layout produced the geometry.
    pub style_layer_id: String,
}

impl Debug for crate::vector::transferables::DefaultSymbolLayerTessellated {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultSymbolLayerTessellated({})", self.coords)
    }
}

impl IntoMessage for crate::vector::transferables::DefaultSymbolLayerTessellated {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl SymbolLayerTessellated for crate::vector::transferables::DefaultSymbolLayerTessellated {
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

    fn build_from(coords: WorldTileCoords, index: TileIndex) -> Self {
        Self { coords, index }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn to_tile_index(self) -> TileIndex {
        self.index
    }
}

/// Selects the message representations used by a vector worker and its matching receiver.
/// Implementations can retain Rust values or serialize them across a platform worker boundary.
pub trait VectorTransferables: Copy + Clone + 'static {
    /// Tile processing completion, including pending-symbol state.
    type TileTessellated: TileTessellated;
    /// A source layer absent from a requested tile.
    type LayerMissing: LayerMissing;
    /// Fill, line or circle geometry and paint metadata.
    type LayerTessellated: LayerTessellated;
    /// Text/icon geometry, atlas and placement metadata.
    type SymbolLayerTessellated: SymbolLayerTessellated;
    /// Tile geometry indexed for queries.
    type LayerIndexed: LayerIndexed;
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

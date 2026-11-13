//! Worker message contracts for vector geometry, symbols and tile query indices.

#![deny(missing_docs)]

mod defaults;

use std::fmt::Debug;

#[cfg(test)]
pub use defaults::{
    DefaultLayerIndexed, DefaultLayerMissing, DefaultLayerTessellated, DefaultTileTessellated,
};
pub use defaults::{DefaultSymbolLayerTessellated, DefaultVectorTransferables};
use geozero::mvt::tile::Layer;

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{IntoMessage, MessageTag},
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
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self
    where
        Self: Sized;

    /// Builds a tessellated layer payload together with its per-entry feature attributes in one
    /// step so single-threaded wire formats can pack everything into a single buffer.
    #[allow(clippy::too_many_arguments)]
    fn build_from_with_paint(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
        entries: Vec<BucketFeatureEntry>,
    ) -> Self
    where
        Self: Sized,
    {
        Self::build_from(
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            layer_data,
            style_layer_id,
        )
        .with_feature_paint(entries)
    }

    /// Attaches per-entry feature attributes for in-place GPU paint and `feature-state` refresh.
    fn with_feature_paint(self, _entries: Vec<BucketFeatureEntry>) -> Self
    where
        Self: Sized,
    {
        self
    }

    /// Extracts any attached per-entry feature attributes before converting into a bucket.
    fn take_feature_paint(&mut self) -> Option<Vec<BucketFeatureEntry>> {
        None
    }

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

    /// Whether this backend transfers the spatial geometry index to the main thread.
    /// Backends that ignore `_index` in [`Self::build_from`] can return `false` to skip
    /// building the query index on the worker thread.
    fn requires_geometry_index() -> bool {
        true
    }

    /// Associates an owned spatial index with its tile-grid coordinates and the source it was
    /// read from.
    fn build_from(coords: WorldTileCoords, source: Option<String>, index: TileIndex) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates associated with the index.
    fn coords(&self) -> WorldTileCoords;

    /// The style's name for the source the index was read from, when the payload carries it.
    fn source(&self) -> Option<String>;

    /// Consumes the transfer representation and returns its spatial index.
    fn to_tile_index(self) -> TileIndex;
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

//! Typed results of headless tile processing: every bucket a worker would send back.
//!
//! Processing runs on the calling thread and needs no renderer, so tessellation can be
//! checked without a GPU. Vector and symbol layers are kept apart, as the world stores them in
//! different tile components.

use std::{collections::BTreeMap, ops::Deref, sync::Arc};

use rustc_hash::FxHashMap;

use crate::{
    coords::WorldTileCoords,
    geojson::{process_geojson_features, GeoJsonTileRequest},
    headless::map::{HeadlessContext, HeadlessMapOperationError},
    io::apc::Message,
    projection::ProjectionType,
    sdf::{
        assets::{fallback_atlas, SymbolAtlas},
        SymbolLayersDataComponent,
    },
    style::{layer::StyleLayer, Style},
    tcs::tiles::Tiles,
    vector::{
        arrival_sync::{
            sync_arriving_symbol_bucket, sync_arriving_tile_index, sync_arriving_vector_bucket,
        },
        feature_paint::TileFeaturePaintComponent,
        process_vector_tile_with_assets,
        transferables::{LayerIndexed, LayerTessellated, SymbolLayerTessellated},
        DefaultVectorTransferables, ProcessVectorContext, VectorLayerBucket,
        VectorLayerBucketComponent, VectorTileRequest, VectorTransferables,
    },
};

/// A tessellated vector layer as the headless worker produces it.
pub type VectorLayer = <DefaultVectorTransferables as VectorTransferables>::LayerTessellated;
/// A tessellated symbol layer as the headless worker produces it.
pub type SymbolLayer = <DefaultVectorTransferables as VectorTransferables>::SymbolLayerTessellated;
/// The query index of a source's tile as the headless worker produces it.
pub type IndexedLayer = <DefaultVectorTransferables as VectorTransferables>::LayerIndexed;

/// Every layer a headless processing step produced, by kind.
#[derive(Debug, Default)]
pub struct ProcessedLayers {
    /// Fill, line, circle and background buckets.
    pub vector: Vec<Box<VectorLayer>>,
    /// Text buckets of symbol layers.
    pub symbols: Vec<Box<SymbolLayer>>,
    /// Feature geometry each source tile indexes for queries.
    pub indexes: Vec<Box<IndexedLayer>>,
}

impl ProcessedLayers {
    /// Moves every layer of `other` into `self`.
    pub fn append(&mut self, other: &mut Self) {
        self.vector.append(&mut other.vector);
        self.symbols.append(&mut other.symbols);
        self.indexes.append(&mut other.indexes);
    }

    /// Whether processing produced no layer of any kind.
    pub fn is_empty(&self) -> bool {
        self.vector.is_empty() && self.symbols.is_empty()
    }

    pub(super) fn insert_into_tiles(
        self,
        style: &Style,
        tiles: &mut Tiles,
    ) -> Result<(), HeadlessMapOperationError> {
        let mut memo = FxHashMap::default();
        for index in self.indexes {
            let coords = index.coords();
            let source = index.source();
            let mut tile_index = (*index).to_tile_index();
            sync_arriving_tile_index(style, source.as_deref(), &mut tile_index, &mut memo);
            tiles.geometry_index.index_tile(&coords, source, tile_index);
        }

        let mut layers_by_tile = BTreeMap::new();
        let mut paint_by_tile: BTreeMap<_, TileFeaturePaintComponent> = BTreeMap::new();
        for mut layer in self.vector {
            let coords = layer.coords;
            let mut entries = layer.take_feature_paint();
            let mut bucket = (*layer).to_bucket();
            let _ = sync_arriving_vector_bucket(style, 0, &mut bucket, &mut entries, &mut memo);
            if let Some(entries) = entries {
                paint_by_tile
                    .entry(coords)
                    .or_default()
                    .committed
                    .insert(bucket.style_layer_id.clone(), entries);
            }
            layers_by_tile
                .entry(coords)
                .or_insert_with(Vec::new)
                .push(VectorLayerBucket::AvailableLayer(bucket));
        }

        for (coords, layers) in layers_by_tile {
            let mut tile = tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?;
            tile.insert(VectorLayerBucketComponent {
                done: true,
                failed: false,
                layers,
                overscaled_zoom: 0,
            });
            if let Some(paint) = paint_by_tile.remove(&coords) {
                tile.insert(paint);
            }
        }

        let mut symbols_by_tile = BTreeMap::new();
        for layer in self.symbols {
            let coords = layer.coords();
            let mut bucket = (*layer).to_bucket();
            let _ = sync_arriving_symbol_bucket(style, &mut bucket);
            symbols_by_tile
                .entry(coords)
                .or_insert_with(Vec::new)
                .push(bucket);
        }
        // A tile whose only layers are symbols is still a delivered tile, as when a worker
        // reports it finished, or its labels would never be covered.
        for (coords, layers) in symbols_by_tile {
            let need_vector = tiles.query::<&VectorLayerBucketComponent>(coords).is_none();
            let mut tile = tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?;
            if need_vector {
                tile.insert(VectorLayerBucketComponent {
                    done: true,
                    failed: false,
                    layers: Vec::new(),
                    overscaled_zoom: 0,
                });
            }
            tile.insert(SymbolLayersDataComponent {
                layers,
                pending_assets: false,
            });
        }
        Ok(())
    }

    fn from_messages(messages: Vec<Message>) -> Result<Self, HeadlessMapOperationError> {
        let mut layers = Self::default();
        for message in messages {
            if message.has_tag(VectorLayer::message_tag()) {
                layers
                    .vector
                    .push(message.into_transferable::<VectorLayer>()?);
            } else if message.has_tag(SymbolLayer::message_tag()) {
                layers
                    .symbols
                    .push(message.into_transferable::<SymbolLayer>()?);
            } else if message.has_tag(IndexedLayer::message_tag()) {
                layers
                    .indexes
                    .push(message.into_transferable::<IndexedLayer>()?);
            }
        }
        Ok(layers)
    }
}

/// Tessellates one vector source tile for a style layer with the bundled fallback glyphs.
pub fn process_tile_layers(
    tile_data: &[u8],
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    process_tile_layers_with_atlas(tile_data, layer, coords, projection, (None, 0))
}

/// Tessellates one vector source tile for a style layer; symbol layers use the atlas for their
/// glyphs and sprites, or the bundled fallback when it is `None`. `overscaled_zoom` is the zoom
/// the tile is magnified to past its source's last zoom, or zero.
pub fn process_tile_layers_with_atlas(
    tile_data: &[u8],
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
    (atlas, overscaled_zoom): (Option<Arc<SymbolAtlas>>, u8),
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    process_tile_layers_with_state(
        tile_data,
        layer,
        coords,
        projection,
        (atlas, overscaled_zoom),
        (None, None),
    )
}

/// Tessellates one vector source tile for `layer` using `style`'s `promoteId` and runtime
/// `feature-state` for `layer.source`.
pub fn process_tile_layers_for_style(
    tile_data: &[u8],
    style: &crate::style::Style,
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
    (atlas, overscaled_zoom): (Option<Arc<SymbolAtlas>>, u8),
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let source_state = layer.source.as_deref().map_or((None, None), |name| {
        (
            style.source_promote_id(name).cloned(),
            style.feature_state.source(name).cloned(),
        )
    });
    process_tile_layers_with_state(
        tile_data,
        layer,
        coords,
        projection,
        (atlas, overscaled_zoom),
        source_state,
    )
}

fn process_tile_layers_with_state(
    tile_data: &[u8],
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
    (atlas, overscaled_zoom): (Option<Arc<SymbolAtlas>>, u8),
    (promote_id, feature_state): (
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, HeadlessContext>::new(
        HeadlessContext::default(),
    )
    .with_source_feature_state(promote_id, feature_state);
    process_vector_tile_with_assets(
        tile_data,
        VectorTileRequest {
            coords,
            layers: [layer].into_iter().cloned().collect(),
            projection,
            overscaled_zoom,
        },
        &mut processor,
        atlas.unwrap_or_else(fallback_atlas),
    )
    .map_err(|source| HeadlessMapOperationError::Vector { source })?;
    let messages = processor.take_context().messages.deref().take();
    ProcessedLayers::from_messages(messages)
}

/// Tessellates inline GeoJSON for the style layers drawing a source.
pub fn process_geojson_layers(
    geojson: &serde_json::Value,
    source_name: &str,
    layers: Vec<StyleLayer>,
    coords: WorldTileCoords,
    projection: ProjectionType,
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    process_geojson_layers_with_atlas(
        geojson,
        source_name,
        layers,
        coords,
        projection,
        (None, 0, None),
    )
}

/// Tessellates inline GeoJSON for the style layers drawing a source; symbol layers use `atlas`
/// for their glyphs and sprites, or the bundled fallback when it is `None`; the tile is drawn
/// magnified to `overscaled_zoom` when that is past its own zoom.
pub fn process_geojson_layers_with_atlas(
    geojson: &serde_json::Value,
    source_name: &str,
    layers: Vec<StyleLayer>,
    coords: WorldTileCoords,
    projection: ProjectionType,
    (atlas, overscaled_zoom, buffer): (Option<Arc<SymbolAtlas>>, u8, Option<u32>),
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let context = HeadlessContext::default();
    process_geojson_features::<DefaultVectorTransferables, HeadlessContext>(
        geojson,
        GeoJsonTileRequest {
            coords,
            layers,
            source_name: source_name.to_owned(),
            projection,
            atlas,
            overscaled_zoom,
            buffer,
        },
        &context,
    )
    .map_err(|source| HeadlessMapOperationError::GeoJson { source })?;
    let messages = context.messages.deref().take();
    ProcessedLayers::from_messages(messages)
}

#[cfg(test)]
mod tests;

//! Typed results of headless tile processing: every bucket a worker would send back.
//!
//! Processing runs on the calling thread and needs no renderer, so tessellation can be
//! checked without a GPU. Vector and symbol layers are kept apart, as the world stores them in
//! different tile components.

use std::{collections::BTreeMap, ops::Deref, sync::Arc};

#[cfg(feature = "geojson")]
use crate::geojson::{process_geojson_features, GeoJsonTileRequest};
use crate::{
    coords::WorldTileCoords,
    headless::map::{HeadlessContext, HeadlessMapOperationError},
    io::apc::Message,
    projection::ProjectionType,
    sdf::{
        assets::{fallback_atlas, SymbolAtlas},
        sync_arriving_symbol_bucket, SymbolLayersDataComponent,
    },
    style::{feature_state::FeatureStateArcMemo, layer::StyleLayer, Style},
    tcs::tiles::Tiles,
    vector::{
        arrival_sync::{sync_arriving_tile_index, sync_arriving_vector_bucket},
        content::TilePropertyInterner,
        feature_paint::TileFeaturePaintComponent,
        process_vector_tile_with_assets,
        transferables::{
            LayerIndexed, LayerTessellated, SymbolLayerTessellated, TileTessellated as _,
        },
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
type TileCompleted = <DefaultVectorTransferables as VectorTransferables>::TileTessellated;

/// Every layer a headless processing step produced, by kind.
#[derive(Debug, Default)]
pub struct ProcessedLayers {
    /// Fill, line, circle and background buckets.
    pub vector: Vec<Box<VectorLayer>>,
    /// Text buckets of symbol layers.
    pub symbols: Vec<Box<SymbolLayer>>,
    /// Feature geometry each source tile indexes for queries.
    pub indexes: Vec<Box<IndexedLayer>>,
    /// Coordinates that completed tessellation (including empty tiles).
    pub completed_tiles: Vec<WorldTileCoords>,
}

impl ProcessedLayers {
    /// Moves every layer of `other` into `self`.
    pub fn append(&mut self, other: &mut Self) {
        self.vector.append(&mut other.vector);
        self.symbols.append(&mut other.symbols);
        self.indexes.append(&mut other.indexes);
        self.completed_tiles.append(&mut other.completed_tiles);
    }

    /// Whether processing produced no layer of any kind.
    pub fn is_empty(&self) -> bool {
        self.vector.is_empty() && self.symbols.is_empty()
    }

    fn insert_vector_layers(
        vector: impl IntoIterator<Item = Box<VectorLayer>>,
        style: &Style,
        tiles: &mut Tiles,
        memo: &mut FeatureStateArcMemo,
        interner: &mut TilePropertyInterner,
    ) -> Result<BTreeMap<WorldTileCoords, u8>, HeadlessMapOperationError> {
        let mut layers_by_tile = BTreeMap::new();
        let mut overscaled_by_tile: BTreeMap<WorldTileCoords, u8> = BTreeMap::new();
        let mut paint_by_tile: BTreeMap<_, TileFeaturePaintComponent> = BTreeMap::new();
        for mut layer in vector {
            let coords = layer.coords;
            let overscaled_zoom = layer.overscaled_zoom();
            overscaled_by_tile
                .entry(coords)
                .and_modify(|zoom| *zoom = (*zoom).max(overscaled_zoom))
                .or_insert(overscaled_zoom);
            let mut entries = layer.take_feature_paint();
            let mut bucket = (*layer).to_bucket();
            let _: bool = sync_arriving_vector_bucket(
                style,
                overscaled_zoom,
                &mut bucket,
                &mut entries,
                memo,
            );
            if let Some(mut entries) = entries.filter(|e| !e.is_empty()) {
                interner.intern_entries(tiles, coords, &mut entries);
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
            let overscaled_zoom = overscaled_by_tile.get(&coords).copied().unwrap_or(0);
            let mut tile = tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?;
            tile.insert(VectorLayerBucketComponent {
                done: true,
                failed: false,
                layers,
                overscaled_zoom,
                uploaded_stamp: None,
            });
            if let Some(paint) = paint_by_tile.remove(&coords) {
                tile.insert(paint);
            }
        }
        Ok(overscaled_by_tile)
    }

    fn insert_symbol_and_completed(
        symbols: impl IntoIterator<Item = Box<SymbolLayer>>,
        completed_tiles: Vec<WorldTileCoords>,
        overscaled_by_tile: &BTreeMap<WorldTileCoords, u8>,
        style: &Style,
        tiles: &mut Tiles,
        memo: &mut FeatureStateArcMemo,
        interner: &mut TilePropertyInterner,
    ) -> Result<(), HeadlessMapOperationError> {
        let mut symbols_by_tile = BTreeMap::new();
        for layer in symbols {
            let coords = layer.coords();
            let mut bucket = (*layer).to_bucket();
            let _: bool = sync_arriving_symbol_bucket(style, &mut bucket, memo);
            interner.intern_symbol_bucket(tiles, &mut bucket);
            symbols_by_tile
                .entry(coords)
                .or_insert_with(Vec::new)
                .push(bucket);
        }
        // A tile whose only layers are symbols is still a delivered tile, as when a worker
        // reports it finished, or its labels would never be covered.
        for (coords, layers) in symbols_by_tile {
            let overscaled_zoom = overscaled_by_tile.get(&coords).copied().unwrap_or(0);
            let need_vector = tiles.query::<&VectorLayerBucketComponent>(coords).is_none();
            let mut tile = tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?;
            if need_vector {
                tile.insert(VectorLayerBucketComponent {
                    done: true,
                    failed: false,
                    layers: Vec::new(),
                    overscaled_zoom,
                    uploaded_stamp: None,
                });
            }
            tile.insert(SymbolLayersDataComponent {
                layers,
                pending_assets: false,
                ..Default::default()
            });
        }
        // Empty tiles (e.g. blank MVT or missing source-layer) emit `TileTessellated` without
        // layers; marking their bucket done suppresses parent fallback.
        for coords in completed_tiles {
            let overscaled_zoom = overscaled_by_tile.get(&coords).copied().unwrap_or(0);
            if let Some(bucket) = tiles.query_mut::<&mut VectorLayerBucketComponent>(coords) {
                bucket.done = true;
                bucket.failed = false;
            } else {
                let mut tile = tiles
                    .spawn_mut(coords)
                    .ok_or(HeadlessMapOperationError::InvalidTile { coords })?;
                tile.insert(VectorLayerBucketComponent {
                    done: true,
                    failed: false,
                    layers: Vec::new(),
                    overscaled_zoom,
                    uploaded_stamp: None,
                });
            }
        }
        Ok(())
    }

    pub(in crate::headless::map) fn insert_into_tiles(
        self,
        style: &Style,
        tiles: &mut Tiles,
    ) -> Result<(), HeadlessMapOperationError> {
        let mut memo = FeatureStateArcMemo::default();
        let mut interner = TilePropertyInterner::default();
        for index in self.indexes {
            let coords = index.coords();
            let source = index.source();
            let mut tile_index = (*index).to_tile_index();
            sync_arriving_tile_index(style, source.as_deref(), &mut tile_index, &mut memo);
            interner.intern_tile_index(tiles, coords, &mut tile_index);
            tiles.as_mut().index_tile(&coords, source, tile_index);
        }

        let overscaled_by_tile =
            Self::insert_vector_layers(self.vector, style, tiles, &mut memo, &mut interner)?;
        Self::insert_symbol_and_completed(
            self.symbols,
            self.completed_tiles,
            &overscaled_by_tile,
            style,
            tiles,
            &mut memo,
            &mut interner,
        )
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
            } else if message.has_tag(TileCompleted::message_tag()) {
                let tess = message.into_transferable::<TileCompleted>()?;
                if !tess.failed() {
                    layers.completed_tiles.push(tess.coords());
                }
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
        (atlas, overscaled_zoom, true),
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
    let has_dem_source = crate::terrain::source::dem_source(style).is_some();
    process_tile_layers_with_state(
        tile_data,
        layer,
        coords,
        projection,
        (atlas, overscaled_zoom, has_dem_source),
        source_state,
    )
}

fn process_tile_layers_with_state(
    tile_data: &[u8],
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
    (atlas, overscaled_zoom, has_dem_source): (Option<Arc<SymbolAtlas>>, u8, bool),
    (promote_id, feature_state): (
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, HeadlessContext>::new(
        HeadlessContext::default(),
    )
    .with_has_dem_source(has_dem_source)
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
#[cfg(feature = "geojson")]
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
#[cfg(feature = "geojson")]
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
            generate_id: false,
            promote_id: None,
        },
        &context,
    )
    .map_err(|source| HeadlessMapOperationError::GeoJson { source })?;
    let messages = context.messages.deref().take();
    ProcessedLayers::from_messages(messages)
}

#[cfg(test)]
mod tests;

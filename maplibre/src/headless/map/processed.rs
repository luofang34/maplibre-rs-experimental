//! Typed results of headless tile processing: every bucket a worker would send back.
//!
//! Processing runs on the calling thread and needs no renderer, so tessellation can be
//! checked without a GPU. Vector and symbol layers are kept apart, as the world stores them in
//! different tile components.

use std::ops::Deref;

use crate::{
    coords::WorldTileCoords,
    geojson::{process_geojson_features, GeoJsonTileRequest},
    headless::map::{HeadlessContext, HeadlessMapOperationError},
    io::apc::Message,
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::{
        process_vector_tile,
        transferables::{LayerTessellated, SymbolLayerTessellated},
        DefaultVectorTransferables, ProcessVectorContext, VectorTileRequest, VectorTransferables,
    },
};

/// A tessellated vector layer as the headless worker produces it.
pub type VectorLayer = <DefaultVectorTransferables as VectorTransferables>::LayerTessellated;
/// A tessellated symbol layer as the headless worker produces it.
pub type SymbolLayer = <DefaultVectorTransferables as VectorTransferables>::SymbolLayerTessellated;

/// Every layer a headless processing step produced, by kind.
#[derive(Debug, Default)]
pub struct ProcessedLayers {
    /// Fill, line, circle and background buckets.
    pub vector: Vec<Box<VectorLayer>>,
    /// Text buckets of symbol layers.
    pub symbols: Vec<Box<SymbolLayer>>,
}

impl ProcessedLayers {
    /// Moves every layer of `other` into `self`.
    pub fn append(&mut self, other: &mut Self) {
        self.vector.append(&mut other.vector);
        self.symbols.append(&mut other.symbols);
    }

    /// Whether processing produced no layer of any kind.
    pub fn is_empty(&self) -> bool {
        self.vector.is_empty() && self.symbols.is_empty()
    }

    fn from_messages(messages: Vec<Message>) -> Self {
        let mut layers = Self::default();
        for message in messages {
            if message.has_tag(VectorLayer::message_tag()) {
                layers
                    .vector
                    .push(message.into_transferable::<VectorLayer>());
            } else if message.has_tag(SymbolLayer::message_tag()) {
                layers
                    .symbols
                    .push(message.into_transferable::<SymbolLayer>());
            }
        }
        layers
    }
}

/// Tessellates one vector source tile for a style layer.
pub fn process_tile_layers(
    tile_data: &[u8],
    layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: ProjectionType,
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, HeadlessContext>::new(
        HeadlessContext::default(),
    );
    process_vector_tile(
        tile_data,
        VectorTileRequest {
            coords,
            layers: [layer].into_iter().cloned().collect(),
            projection,
        },
        &mut processor,
    )
    .map_err(|source| HeadlessMapOperationError::Vector { source })?;
    let messages = processor.take_context().messages.deref().take();
    Ok(ProcessedLayers::from_messages(messages))
}

/// Tessellates inline GeoJSON for the style layers drawing a source.
pub fn process_geojson_layers(
    geojson: &serde_json::Value,
    source_name: &str,
    layers: Vec<StyleLayer>,
    coords: WorldTileCoords,
    projection: ProjectionType,
) -> Result<ProcessedLayers, HeadlessMapOperationError> {
    let context = HeadlessContext::default();
    process_geojson_features::<DefaultVectorTransferables, HeadlessContext>(
        geojson,
        GeoJsonTileRequest {
            coords,
            layers,
            source_name: source_name.to_owned(),
            projection,
        },
        &context,
    )
    .map_err(|source| HeadlessMapOperationError::GeoJson { source })?;
    let messages = context.messages.deref().take();
    Ok(ProcessedLayers::from_messages(messages))
}

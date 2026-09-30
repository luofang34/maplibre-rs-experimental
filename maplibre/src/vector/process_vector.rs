//! Vector decoding, style evaluation and worker result production.

#![deny(missing_docs)]

use std::{collections::HashSet, marker::PhantomData};

use geozero::{
    mvt::{tile, Message},
    GeozeroDatasource,
};
use thiserror::Error;

use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::{
        apc::{Context, SendError},
        geometry_index::{IndexProcessor, IndexedGeometry, TileIndex},
    },
    projection::{globe::subdivision::granularity_for_zoom, ProjectionType},
    render::ShaderVertex,
    sdf::{assets::SymbolAssetError, tessellation::TextTessellator},
    style::{
        expression::{FeatureProperties, Value},
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        tessellation::{
            CircleOptions, ExtrusionOptions, IndexDataType, OverAlignedVertexBuffer,
            ZeroTessellator,
        },
        transferables::{
            LayerIndexed, LayerMissing, LayerTessellated, SymbolLayerTessellated, TileTessellated,
            VectorTransferables,
        },
    },
};

/// Failure decoding vector tile bytes or returning processed results.
#[derive(Error, Debug)]
pub enum ProcessVectorError {
    /// Sending of results failed
    #[error("sending data back through context failed")]
    SendError(#[source] SendError),
    /// The source bytes are not a decodable vector tile.
    #[error("decoding vector tile {coords} failed: {source}")]
    Decoding {
        /// Tile whose protocol buffer could not be decoded.
        coords: WorldTileCoords,
        /// Underlying protocol-buffer decoder failure.
        #[source]
        source: Box<dyn std::error::Error>,
    },
    /// A glyph range or sprite could not be fetched, so the tile's symbols would be incomplete.
    #[error("loading symbol assets failed")]
    SymbolAssets(#[source] SymbolAssetError),
}

/// Scale from a layer's declared coordinate extent to the 4096 grid the shaders expect, so a
/// producer emitting 8192 keeps its precision instead of landing twice as far from the origin.
pub fn extent_scale(layer: &tile::Layer) -> f64 {
    match layer.extent {
        Some(extent) if extent > 0 => EXTENT / f64::from(extent),
        _ => 1.0,
    }
}

/// A request for a tile at the given coordinates and in the given layers.
pub struct VectorTileRequest {
    /// Tile grid used to scale its geometry and index.
    pub coords: WorldTileCoords,
    /// Style entries to evaluate against the source layers.
    pub layers: HashSet<StyleLayer>,
    /// Projection controlling globe subdivision of tile geometry.
    pub projection: ProjectionType,
}

/// Reads an MVT feature's tags into typed filter values through the layer's key and value
/// tables.
pub(crate) fn feature_properties(
    layer: &tile::Layer,
    feature: &tile::Feature,
) -> FeatureProperties {
    let mut properties = FeatureProperties::new();
    for pair in feature.tags.chunks(2) {
        let [key_index, value_index] = pair else {
            continue;
        };
        let (Some(key), Some(value)) = (
            layer.keys.get(*key_index as usize),
            layer.values.get(*value_index as usize),
        ) else {
            continue;
        };
        let value = if let Some(text) = &value.string_value {
            Value::String(text.clone())
        } else if let Some(number) = value.float_value {
            Value::Number(f64::from(number))
        } else if let Some(number) = value.double_value {
            Value::Number(number)
        } else if let Some(number) = value.int_value {
            Value::Number(number as f64)
        } else if let Some(number) = value.uint_value {
            Value::Number(number as f64)
        } else if let Some(number) = value.sint_value {
            Value::Number(number as f64)
        } else if let Some(flag) = value.bool_value {
            Value::Bool(flag)
        } else {
            continue;
        };
        properties.insert(key.clone(), value);
    }
    properties
}

/// Keeps only the features of an MVT layer that pass the filter.
fn apply_filter_to_layer(layer: &mut tile::Layer, filter: &Filter, zoom: f64) {
    let keep: Vec<bool> = layer
        .features
        .iter()
        .map(|feature| {
            let properties = feature_properties(layer, feature);
            filter.evaluate(&FeatureContext {
                properties: &properties,
                geometry_type: GeometryType::from_mvt(feature.r#type.unwrap_or_default()),
                id: feature.id.map(|id| Value::Number(id as f64)),
                zoom,
            })
        })
        .collect();
    let mut keep = keep.into_iter();
    layer.features.retain(|_| keep.next().unwrap_or(false));
}

/// Decodes and processes a tile with the built-in fallback symbol atlas.
/// Results stream through the context; decoding and delivery failures retain their causes.
pub fn process_vector_tile<T: VectorTransferables, C: Context>(
    data: &[u8],
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    process_vector_tile_with_assets(
        data,
        tile_request,
        context,
        crate::sdf::assets::fallback_atlas(),
    )
}

mod processing;

/// Decodes a tile, tolerating the zero padding some tile servers append: a zero tag is not a
/// field, so a strict decoder rejects a tile that GL JS reads.
fn decode_tile(data: &[u8]) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error>> {
    geozero::mvt::Tile::decode(data).or_else(|error| {
        let end = data.iter().rposition(|byte| *byte != 0).map_or(0, |i| i + 1);
        if end == data.len() {
            return Err(error.into());
        }
        geozero::mvt::Tile::decode(&data[..end]).map_err(|_| error.into())
    })
}

pub(crate) fn process_vector_tile_with_assets<T: VectorTransferables, C: Context>(
    data: &[u8],
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    let mut tile = decode_tile(data).map_err(|source| ProcessVectorError::Decoding {
        coords: tile_request.coords,
        source,
    })?;
    for style in &tile_request.layers {
        let (Some(_), Some(name)) = (&style.paint, &style.source_layer) else {
            tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
            continue;
        };
        // Omitted source layers must clear successful replacements just like empty geometry.
        let layer = tile
            .layers
            .iter()
            .find(|layer| &layer.name == name)
            .cloned()
            .unwrap_or_else(|| tile::Layer {
                version: 2,
                name: name.clone(),
                extent: Some(4096),
                ..Default::default()
            });
        processing::process_layer(layer, style, &tile_request, context, atlas.clone())?;
    }
    let mut index = IndexProcessor::new();
    for layer in &mut tile.layers {
        index.set_coordinate_scale(extent_scale(layer));
        index.begin_layer(
            &layer.name,
            layer.features.iter().map(|feature| feature.id).collect(),
        );
        // Query decoding cannot make successfully rendered geometry unavailable.
        if let Err(error) = layer.process(&mut index) {
            tracing::warn!(coords = %tile_request.coords, layer = %layer.name, ?error, "skipping query index for layer");
        }
    }
    let source = tile_request
        .layers
        .iter()
        .find_map(|layer| layer.source.clone());
    context.layer_indexing_finished(&tile_request.coords, source, index.get_geometries())?;
    context.tile_finished(&tile_request.coords)?;
    Ok(())
}

enum Completion {
    Deferred,
    PendingSymbols,
    Finished,
}

/// Reply context and completion mode used while processing vector layers.
pub struct ProcessVectorContext<T: VectorTransferables, C: Context> {
    context: C,
    completion: Completion,
    phantom_t: PhantomData<T>,
}

impl<T: VectorTransferables, C: Context> ProcessVectorContext<T, C> {
    /// Uses the supplied reply endpoint and completes the tile after this processing batch.
    pub fn new(context: C) -> Self {
        Self {
            context,
            completion: Completion::Finished,
            phantom_t: Default::default(),
        }
    }
}

impl<T: VectorTransferables, C: Context> ProcessVectorContext<T, C> {
    pub(crate) fn with_pending_symbols(mut self) -> Self {
        self.completion = Completion::PendingSymbols;
        self
    }

    pub(crate) fn without_completion(mut self) -> Self {
        self.completion = Completion::Deferred;
        self
    }

    /// Returns the owned reply endpoint after processing.
    pub fn take_context(self) -> C {
        self.context
    }

    fn tile_finished(&mut self, coords: &WorldTileCoords) -> Result<(), ProcessVectorError> {
        let message = match self.completion {
            Completion::Deferred => return Ok(()),
            Completion::PendingSymbols => T::TileTessellated::build_partial(*coords),
            Completion::Finished => T::TileTessellated::build_from(*coords),
        };
        self.context
            .send_back(message)
            .map_err(ProcessVectorError::SendError)
    }

    fn layer_missing(
        &mut self,
        coords: &WorldTileCoords,
        layer_name: &str,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerMissing::build_from(*coords, layer_name.to_owned()))
            .map_err(ProcessVectorError::SendError)
    }

    fn layer_tessellation_finished(
        &mut self,
        coords: &WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        layer_data: tile::Layer,
        style_layer_id: String,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerTessellated::build_from(
                *coords,
                buffer,
                feature_indices,
                feature_colors,
                layer_data,
                style_layer_id,
            ))
            .map_err(ProcessVectorError::SendError)
    }

    fn symbol_layer_tessellation_finished(
        &mut self,
        layer: crate::vector::transferables::DefaultSymbolLayerTessellated,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::SymbolLayerTessellated::build_from(
                layer.coords,
                layer.buffer,
                layer.features,
                layer.atlas,
                layer.layer_data,
                layer.style_layer_id,
            ))
            .map_err(ProcessVectorError::SendError)
    }

    fn layer_indexing_finished(
        &mut self,
        coords: &WorldTileCoords,
        source: Option<String>,
        geometries: Vec<IndexedGeometry<f64>>,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerIndexed::build_from(
                *coords,
                source,
                TileIndex::Linear { list: geometries },
            ))
            .map_err(ProcessVectorError::SendError)
    }
}

#[cfg(test)]
mod tests;

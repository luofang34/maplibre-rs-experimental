//! Vector decoding, style evaluation and worker result production.

#![deny(missing_docs)]

use std::marker::PhantomData;

use geozero::mvt::tile;
use rustc_hash::FxHashSet as HashSet;
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
        filter::Filter,
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
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A glyph range or sprite could not be fetched, so the tile's symbols would be incomplete.
    #[error("loading symbol assets failed")]
    SymbolAssets(#[source] SymbolAssetError),
    /// Tessellation did not run to completion.
    #[error("processing vector tile failed")]
    CpuWork(#[source] crate::io::cpu_work::CpuWorkError),
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
#[derive(Clone)]
pub struct VectorTileRequest {
    /// Tile grid used to scale its geometry and index.
    pub coords: WorldTileCoords,
    /// Style entries to evaluate against the source layers.
    pub layers: HashSet<StyleLayer>,
    /// Projection controlling globe subdivision of tile geometry.
    pub projection: ProjectionType,
    /// The zoom the tile is drawn at when that is past the last zoom of its source, so its
    /// geometry is magnified; zero when the tile is drawn at its own zoom.
    pub overscaled_zoom: u8,
}

impl VectorTileRequest {
    /// The zoom style expressions are evaluated at: the zoom the tile is magnified to, which is
    /// the tile's own when it is not.
    pub fn style_zoom(&self) -> f64 {
        f64::from(self.overscaled_zoom.max(u8::from(self.coords.z)))
    }

    /// How many times the tile is magnified past its source's last zoom.
    pub fn overscaling(&self) -> f64 {
        let own = u8::from(self.coords.z);
        2_f64.powi(i32::from(self.overscaled_zoom.saturating_sub(own)))
    }
}

/// Reads an MVT feature's tags into typed filter values through the layer's key and value
/// tables.
pub(crate) fn feature_properties(
    layer: &tile::Layer,
    feature: &tile::Feature,
) -> FeatureProperties {
    let mut properties = FeatureProperties::default();
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

mod decode;
mod geometry_tags;
mod mvt_slice;
mod processing;
mod shared_geometry;
pub(crate) use decode::{decode_tile, decode_tile_for_layers};
pub(crate) use mvt_slice::index_mvt_bytes;
#[cfg(feature = "mlt")]
pub(crate) use mvt_slice::looks_like_mvt;
#[cfg(feature = "mlt")]
pub(crate) use processing::{
    process_layer as process_layer_from_mvt, tessellator as build_zero_tessellator,
};

/// Pre-decoded MVT protobuf or validated zero-copy MLT columnar byte slice shared across base and
/// symbol passes of a tile worker.
pub(crate) enum DecodedVectorTile {
    Mvt(geozero::mvt::Tile),
    #[cfg(feature = "mlt")]
    Mlt(std::sync::Arc<[u8]>),
}

pub(crate) fn process_vector_tile_with_assets<T: VectorTransferables, C: Context>(
    data: &[u8],
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    #[cfg(feature = "mlt")]
    if !looks_like_mvt(data) && super::mlt::is_valid_mlt(data) {
        return super::mlt::process_mlt_tile_with_assets(data, tile_request, context, atlas);
    }
    let decode_res = if context.index_geometry && T::LayerIndexed::requires_geometry_index() {
        decode_tile(data)
    } else {
        let keep = |name: &str| {
            tile_request
                .layers
                .iter()
                .any(|sl| sl.source_layer.as_deref() == Some(name))
        };
        decode_tile_for_layers(data, keep)
    };
    let tile = decode_res.map_err(|source| ProcessVectorError::Decoding {
        coords: tile_request.coords,
        source,
    })?;
    process_mvt_tile_with_assets(&tile, tile_request, context, atlas)
}

pub(crate) fn process_decoded_vector_tile_with_assets<T: VectorTransferables, C: Context>(
    decoded: &DecodedVectorTile,
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    match decoded {
        DecodedVectorTile::Mvt(tile) => {
            process_mvt_tile_with_assets(tile, tile_request, context, atlas)
        }
        #[cfg(feature = "mlt")]
        DecodedVectorTile::Mlt(bytes) => {
            super::mlt::process_mlt_tile_with_assets(bytes, tile_request, context, atlas)
        }
    }
}

fn process_mvt_tile_with_assets<T: VectorTransferables, C: Context>(
    tile: &geozero::mvt::Tile,
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    context.set_overscaled_zoom(tile_request.overscaled_zoom);
    let gpu_atlas = atlas.gpu_upload_handle();
    let index_uses_props = context.index_geometry && T::LayerIndexed::requires_geometry_index();
    let mut ordered_layers: Vec<&StyleLayer> = tile_request.layers.iter().collect();
    ordered_layers.sort_by(|a, b| {
        a.source_layer
            .as_deref()
            .unwrap_or("")
            .cmp(b.source_layer.as_deref().unwrap_or(""))
            .then_with(|| a.index.cmp(&b.index))
    });
    let mut shared_layer_props: rustc_hash::FxHashMap<
        &str,
        Option<Vec<std::sync::Arc<FeatureProperties>>>,
    > = rustc_hash::FxHashMap::default();
    shared_geometry::process_ordered_layers(
        tile,
        &ordered_layers,
        index_uses_props,
        &tile_request,
        context,
        (&atlas, &gpu_atlas),
        &mut shared_layer_props,
    )?;
    if context.index_geometry {
        let geometries = if T::LayerIndexed::requires_geometry_index() {
            let mut index = IndexProcessor::new();
            for layer in &tile.layers {
                index.set_coordinate_scale(extent_scale(layer));
                let state_ctx = context.state_context_for_layer(&layer.name);
                let shared = shared_layer_props
                    .remove(layer.name.as_str())
                    .flatten()
                    .or_else(|| mvt_slice::decode_layer_properties(layer, state_ctx.as_ref()));
                index.set_feature_state_context(state_ctx);
                index.begin_layer(
                    &layer.name,
                    layer.features.iter().map(|feature| feature.id).collect(),
                );
                let skip_props = shared.is_some();
                if let Some(props) = shared {
                    index.set_shared_properties(props);
                }
                // Query decoding cannot make successfully rendered geometry unavailable.
                if let Err(error) =
                    mvt_slice::process_mvt_slice(layer, None, skip_props, &mut index)
                {
                    tracing::warn!(coords = %tile_request.coords, layer = %layer.name, ?error, "skipping query index for layer");
                }
            }
            index.get_geometries()
        } else {
            Vec::new()
        };
        let source = tile_request
            .layers
            .iter()
            .find_map(|layer| layer.source.clone());
        context.layer_indexing_finished(&tile_request.coords, source, geometries)?;
    }
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
    index_geometry: bool,
    overscaled_zoom: u8,
    has_dem_source: bool,
    promote_id: Option<crate::style::source::PromoteId>,
    feature_state: Option<std::sync::Arc<crate::style::feature_state::SourceFeatureState>>,
    phantom_t: PhantomData<T>,
}

impl<T: VectorTransferables, C: Context> ProcessVectorContext<T, C> {
    /// Uses the supplied reply endpoint and completes the tile after this processing batch.
    pub fn new(context: C) -> Self {
        Self {
            context,
            completion: Completion::Finished,
            index_geometry: true,
            overscaled_zoom: 0,
            has_dem_source: false,
            promote_id: None,
            feature_state: None,
            phantom_t: Default::default(),
        }
    }

    pub(crate) fn set_overscaled_zoom(&mut self, overscaled_zoom: u8) {
        self.overscaled_zoom = overscaled_zoom;
    }

    pub(crate) fn with_has_dem_source(mut self, has_dem_source: bool) -> Self {
        self.has_dem_source = has_dem_source;
        self
    }

    /// Configures `promoteId` and active runtime `feature-state` for the tile's source.
    pub fn with_source_feature_state(
        mut self,
        promote_id: Option<crate::style::source::PromoteId>,
        feature_state: Option<crate::style::feature_state::SourceFeatureState>,
    ) -> Self {
        self.promote_id = promote_id;
        self.feature_state = feature_state.map(std::sync::Arc::new);
        self
    }

    pub(crate) fn state_context_for_layer(
        &self,
        source_layer: &str,
    ) -> Option<crate::vector::tessellation::feature_paint::FeatureStateSyncContext> {
        if self.promote_id.is_none() && self.feature_state.is_none() {
            return None;
        }
        Some(
            crate::vector::tessellation::feature_paint::FeatureStateSyncContext {
                promote_property: self
                    .promote_id
                    .as_ref()
                    .and_then(|p| p.property_for(Some(source_layer)))
                    .map(str::to_owned),
                source_state: self.feature_state.clone(),
                source_layer: Some(source_layer.to_owned()),
            },
        )
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

    pub(crate) fn without_indexing(mut self) -> Self {
        self.index_geometry = false;
        self
    }

    #[cfg(feature = "mlt")]
    pub(crate) fn index_geometry(&self) -> bool {
        self.index_geometry
    }

    /// Returns the owned reply endpoint after processing.
    pub fn take_context(self) -> C {
        self.context
    }

    pub(crate) fn tile_finished(
        &mut self,
        coords: &WorldTileCoords,
    ) -> Result<(), ProcessVectorError> {
        let message = match self.completion {
            Completion::Deferred => return Ok(()),
            Completion::PendingSymbols => T::TileTessellated::build_partial(*coords),
            Completion::Finished => T::TileTessellated::build_from(*coords),
        };
        self.context
            .send_back(message)
            .map_err(ProcessVectorError::SendError)
    }

    pub(crate) fn layer_missing(
        &mut self,
        coords: &WorldTileCoords,
        layer_name: &str,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerMissing::build_from(*coords, layer_name.to_owned()))
            .map_err(ProcessVectorError::SendError)
    }

    pub(crate) fn layer_tessellation_finished(
        &mut self,
        coords: &WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        (feature_colors, feature_sort_keys, feature_paint): (
            Vec<[f32; 4]>,
            Vec<f32>,
            Vec<crate::vector::feature_paint::BucketFeatureEntry>,
        ),
        layer_data: tile::Layer,
        style: &crate::style::layer::StyleLayer,
    ) -> Result<(), ProcessVectorError> {
        let overscaled = self.overscaled_zoom.max(u8::from(coords.z));
        let per_feature_pattern = style
            .paint
            .as_ref()
            .and_then(crate::vector::pattern::per_feature_pattern)
            .is_some();
        let allow_prepack = cfg!(target_arch = "wasm32")
            && !self.has_dem_source
            && !per_feature_pattern
            && feature_sort_keys.is_empty()
            && style.type_ != "circle"
            && crate::vector::structures::kind(style)
                .is_none_or(|k| matches!(k, crate::vector::structures::StructureKind::Extrusion))
            && buffer.shared_geometry_key().is_none();
        let msg = T::LayerTessellated::build_from_with_paint_typed(
            *coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            layer_data,
            style.id.clone(),
            feature_paint,
            style.type_.as_str(),
            allow_prepack,
        )
        .with_overscaled_zoom(overscaled);
        self.context
            .send_back(msg)
            .map_err(ProcessVectorError::SendError)
    }

    pub(crate) fn symbol_layer_tessellation_finished(
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

    pub(crate) fn layer_indexing_finished(
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

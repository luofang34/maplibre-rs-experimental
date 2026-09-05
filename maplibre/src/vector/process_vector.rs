use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    marker::PhantomData,
};

use geozero::{
    mvt::{tile, Message},
    GeozeroDatasource,
};
use thiserror::Error;

use serde_json::Value;

use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::{
        apc::{Context, SendError},
        geometry_index::{IndexProcessor, IndexedGeometry, TileIndex},
    },
    projection::{globe::subdivision::granularity_for_zoom, ProjectionType},
    render::{
        shaders::{ShaderSymbolVertex, ShaderSymbolVertexNew},
        ShaderVertex,
    },
    sdf::{tessellation::TextTessellator, tessellation_new::TextTessellatorNew, Feature},
    style::{
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        tessellation::{CircleOptions, IndexDataType, OverAlignedVertexBuffer, ZeroTessellator},
        transferables::{
            LayerIndexed, LayerMissing, LayerTessellated, SymbolLayerTessellated, TileTessellated,
            VectorTransferables,
        },
    },
};

#[derive(Error, Debug)]
pub enum ProcessVectorError {
    /// Sending of results failed
    #[error("sending data back through context failed")]
    SendError(SendError),
    /// Error when decoding e.g. the protobuf file
    #[error("decoding failed")]
    Decoding(Cow<'static, str>),
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
    pub coords: WorldTileCoords,
    pub layers: HashSet<StyleLayer>,
    pub projection: ProjectionType,
}

/// Reads an MVT feature's tags into typed filter values through the layer's key and value
/// tables.
fn feature_properties(layer: &tile::Layer, feature: &tile::Feature) -> HashMap<String, Value> {
    let mut properties = HashMap::new();
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
            Value::from(number)
        } else if let Some(number) = value.double_value {
            Value::from(number)
        } else if let Some(number) = value.int_value {
            Value::from(number)
        } else if let Some(number) = value.uint_value {
            Value::from(number)
        } else if let Some(number) = value.sint_value {
            Value::from(number)
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
                id: feature.id.map(Value::from),
                zoom,
            })
        })
        .collect();
    let mut keep = keep.into_iter();
    layer.features.retain(|_| keep.next().unwrap_or(false));
}

pub fn process_vector_tile<T: VectorTransferables, C: Context>(
    data: &[u8],
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let mut tile = geozero::mvt::Tile::decode(data)
        .map_err(|e| ProcessVectorError::Decoding(e.to_string().into()))?;

    // Report available layers
    let coords = &tile_request.coords;

    for style_layer in &tile_request.layers {
        let id = &style_layer.id;
        if let (Some(paint), Some(source_layer)) = (&style_layer.paint, &style_layer.source_layer) {
            if let Some(layer) = tile
                .layers
                .iter_mut()
                .find(|layer| &layer.name == source_layer)
            {
                // Clone the layer so filtering doesn't affect other style layers
                // that reference the same source layer.
                let mut filtered_layer = layer.clone();

                if let Some(filter) = &style_layer.filter {
                    match Filter::parse(filter) {
                        Ok(filter) => apply_filter_to_layer(
                            &mut filtered_layer,
                            &filter,
                            f64::from(u8::from(coords.z)),
                        ),
                        Err(error) => {
                            // Rendering every feature or none would both be wrong; nothing
                            // plus a loud error is the one a style author can act on.
                            tracing::error!(
                                layer = %id,
                                %error,
                                "unsupported filter; the layer renders nothing"
                            );
                            context.layer_missing(coords, source_layer)?;
                            continue;
                        }
                    }
                }

                let original_layer = filtered_layer.clone();
                let layer = &mut filtered_layer;
                let coordinate_scale = extent_scale(layer);

                match paint {
                    LayerPaint::Line(_) | LayerPaint::Fill(_) | LayerPaint::Circle(_) => {
                        let granularity = match paint {
                            LayerPaint::Fill(_) => {
                                granularity_for_zoom(128, 2, u8::from(tile_request.coords.z))
                            }
                            LayerPaint::Line(_) => {
                                granularity_for_zoom(512, 0, u8::from(tile_request.coords.z))
                            }
                            _ => 1,
                        };
                        let use_globe_geometry = tile_request
                            .projection
                            .uses_globe_rendering(f64::from(u8::from(tile_request.coords.z)));
                        let mut tessellator = match paint {
                            LayerPaint::Circle(circle) => {
                                ZeroTessellator::<IndexDataType>::default().with_circles(
                                    CircleOptions::for_paint(
                                        circle,
                                        f64::from(u8::from(tile_request.coords.z)),
                                    ),
                                )
                            }
                            _ if use_globe_geometry => {
                                let zoom = usize::from(u8::from(tile_request.coords.z));
                                let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[zoom]) - 1;
                                ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(
                                    granularity,
                                    u8::from(tile_request.coords.z) == 0,
                                    tile_request.coords.y == 0,
                                    i64::from(tile_request.coords.y) == last_tile,
                                )
                            }
                            _ => ZeroTessellator::<IndexDataType>::default(),
                        };
                        tessellator.coordinate_scale = coordinate_scale;
                        match paint {
                            LayerPaint::Fill(p) => {
                                tessellator.style_property = p.fill_color.clone()
                            }
                            LayerPaint::Circle(p) => {
                                tessellator.style_property = p.circle_color.clone()
                            }
                            LayerPaint::Line(p) => {
                                tessellator.style_property = p.line_color.clone();
                                tessellator.is_line_layer = true;
                            }
                            LayerPaint::Background(p) => {
                                tessellator.style_property = p.background_color.clone()
                            }
                            _ => {}
                        }

                        if let Err(e) = layer.process(&mut tessellator) {
                            context.layer_missing(coords, &source_layer)?;

                            tracing::error!("tessellation for layer source {source_layer} at {coords} failed {e:?}");
                        } else {
                            context.layer_tessellation_finished(
                                coords,
                                tessellator.buffer.into(),
                                tessellator.feature_indices,
                                tessellator.feature_colors,
                                original_layer,
                                id.clone(),
                            )?;
                        }
                    }
                    LayerPaint::Symbol(symbol_paint) => {
                        let mut tessellator = TextTessellator::<IndexDataType>::default();
                        let text_field = symbol_paint
                            .text_field
                            .clone()
                            .unwrap_or_else(|| "name".to_string());
                        let mut tessellator_new = TextTessellatorNew::new(text_field);
                        tessellator_new.coordinate_scale = coordinate_scale;

                        if let Err(e) = layer.process(&mut tessellator_new) {
                            context.layer_missing(coords, &source_layer)?;

                            tracing::error!("tessellation for layer source {source_layer} at {coords} failed {e:?}");
                        } else {
                            tessellator_new.finish();
                            context.symbol_layer_tessellation_finished(
                                coords,
                                tessellator.quad_buffer.into(),
                                tessellator_new.quad_buffer.into(),
                                tessellator_new.features,
                                original_layer,
                                id.clone(),
                            )?;
                        }
                    }
                    _ => {
                        log::warn!("unhandled style layer type in {id}");
                    }
                }
            } else {
                // A tile without the layer is routine: producers omit empty layers.
                tracing::debug!(%coords, layer = %source_layer, "source layer absent from the tile");
            }
        } else {
            log::error!("vector style layer {id} misses a required attribute");
        }
    }

    // Report missing layers
    let coords = &tile_request.coords;
    let available_layers: HashSet<_> = tile
        .layers
        .iter()
        .map(|layer| layer.name.clone())
        .collect::<HashSet<_>>();

    for layer in tile_request.layers {
        if let Some(source_layer) = layer.source_layer {
            if !available_layers.contains(&source_layer) {
                context.layer_missing(coords, &source_layer)?;
                tracing::info!(
                    "requested source layer {source_layer} at {coords} not found in tile"
                );
            }
        }
    }

    // Report index for layer
    let mut index = IndexProcessor::new();

    for layer in &mut tile.layers {
        index.set_coordinate_scale(extent_scale(layer));
        // A layer that the index cannot decode still rendered above; losing its query index is
        // better than losing the worker.
        if let Err(error) = layer.process(&mut index) {
            tracing::warn!(%coords, layer = %layer.name, ?error, "skipping query index for layer");
        }
    }

    context.layer_indexing_finished(&tile_request.coords, index.get_geometries())?;

    // Report end
    tracing::info!("tile tessellated at {coords} finished");
    context.tile_finished(coords)?;

    Ok(())
}

pub struct ProcessVectorContext<T: VectorTransferables, C: Context> {
    context: C,
    phantom_t: PhantomData<T>,
}

impl<T: VectorTransferables, C: Context> ProcessVectorContext<T, C> {
    pub fn new(context: C) -> Self {
        Self {
            context,
            phantom_t: Default::default(),
        }
    }
}

impl<T: VectorTransferables, C: Context> ProcessVectorContext<T, C> {
    pub fn take_context(self) -> C {
        self.context
    }

    fn tile_finished(&mut self, coords: &WorldTileCoords) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::TileTessellated::build_from(*coords))
            .map_err(|e| ProcessVectorError::SendError(e))
    }

    fn layer_missing(
        &mut self,
        coords: &WorldTileCoords,
        layer_name: &str,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerMissing::build_from(*coords, layer_name.to_owned()))
            .map_err(|e| ProcessVectorError::SendError(e))
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
            .map_err(|e| ProcessVectorError::SendError(e))
    }

    fn symbol_layer_tessellation_finished(
        &mut self,
        coords: &WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        new_buffer: OverAlignedVertexBuffer<ShaderSymbolVertexNew, IndexDataType>,
        features: Vec<Feature>,
        layer_data: tile::Layer,
        style_layer_id: String,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::SymbolLayerTessellated::build_from(
                *coords,
                buffer,
                new_buffer,
                features,
                layer_data,
                style_layer_id,
            ))
            .map_err(|e| ProcessVectorError::SendError(e))
    }

    fn layer_indexing_finished(
        &mut self,
        coords: &WorldTileCoords,
        geometries: Vec<IndexedGeometry<f64>>,
    ) -> Result<(), ProcessVectorError> {
        self.context
            .send_back(T::LayerIndexed::build_from(
                *coords,
                TileIndex::Linear { list: geometries },
            ))
            .map_err(|e| ProcessVectorError::SendError(e))
    }
}

#[cfg(test)]
mod tests;

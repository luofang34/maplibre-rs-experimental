//! Places elevated text and icons once for both eyes and uploads only changed metadata.
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
};

#[cfg(test)]
use super::placement::canonical_tile;
use super::{
    collision_grid::CollisionGrid,
    paint::SymbolUniforms,
    placement::{screen_boxes, symbol_elevation},
    query::{PlacedSymbol, PlacedSymbols},
};
use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::projection_data_for_view,
        shaders::SDFShaderFeatureMetadata,
    },
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    style::layer::LayerPaint,
    tcs::system::{System, SystemError, SystemResult},
};

#[derive(Default)]
pub struct CollisionSystem {
    runs: u32,
    history: temporal::PlacementHistory,
    uploaded: HashMap<(WorldTileCoords, String), (u64, u64)>,
}

impl CollisionSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

fn opacity_fingerprint(metadata: &[SDFShaderFeatureMetadata]) -> u64 {
    let mut hash = DefaultHasher::new();
    for entry in metadata {
        entry.opacity.to_bits().hash(&mut hash);
        entry.elevation.to_bits().hash(&mut hash);
    }
    hash.finish()
}

impl System for CollisionSystem {
    fn name(&self) -> Cow<'static, str> {
        "sdf_collision_system".into()
    }

    fn run(
        &mut self,
        MapContext {
            world,
            style,
            view_state,
            renderer,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
            return Ok(());
        }
        self.runs = self.runs.wrapping_add(1);
        let seen: HashSet<_> = world
            .resources
            .get::<super::covering::SymbolCovering>()
            .map(|covering| covering.tiles.iter().copied().collect())
            .unwrap_or_default();
        let mut layers = visible_layers(world, style, view_state.style_zoom().value(), &seen);
        let new_content = layers.iter().any(|(_, layer, _)| {
            allocation(world, layer.coords, &layer.style_layer_id)
                != self
                    .uploaded
                    .get(&(layer.coords, layer.style_layer_id.clone()))
                    .map(|entry| entry.0)
        });
        if !new_content && !self.history.fading && !self.runs.is_multiple_of(8) {
            return Ok(());
        }
        layers.sort_by_key(|(index, layer, _)| {
            (
                std::cmp::Reverse(*index),
                std::cmp::Reverse(u8::from(layer.coords.z)),
                layer.coords.y,
                layer.coords.x,
            )
        });
        let projection = projection_data_for_view(style, view_state).map_err(|error| {
            tracing::error!(%error, "symbol projection failed");
            SystemError::Setup
        })?;
        self.history.begin(
            world
                .resources
                .get::<crate::render::frame_input::FrameInput>()
                .map_or(std::time::Duration::ZERO, |input| input.timestamp),
        );
        let placed = self.place_layers(
            world,
            style,
            view_state,
            &projection,
            &renderer.queue,
            layers,
        );
        self.uploaded.retain(|(coords, _), _| seen.contains(coords));
        world.resources.insert(placed);
        Ok(())
    }
}

mod rules;
mod temporal;

#[cfg(test)]
mod tests;

fn visible_layers<'a>(
    world: &'a crate::tcs::world::World,
    style: &'a crate::style::Style,
    zoom: f64,
    seen: &HashSet<WorldTileCoords>,
) -> Vec<(
    u32,
    &'a crate::sdf::SymbolLayerData,
    &'a crate::style::layer::SymbolPaint,
)> {
    let mut layers = Vec::new();
    for coords in seen {
        if let Some(component) = world.tiles.query::<&SymbolLayersDataComponent>(*coords) {
            for layer in &component.layers {
                if let Some(style_layer) = style
                    .layers
                    .iter()
                    .find(|style| style.id == layer.style_layer_id && style.is_visible_at(zoom))
                {
                    if let Some(LayerPaint::Symbol(paint)) = &style_layer.paint {
                        layers.push((style_layer.index, layer, paint));
                    }
                }
            }
        }
    }
    layers
}

fn place_layer(
    world: &crate::tcs::world::World,
    view_state: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    layer: &crate::sdf::SymbolLayerData,
    paint: &crate::style::layer::SymbolPaint,
    zoom_limits: [f64; 2],
    placement: (
        &mut CollisionGrid,
        &mut PlacedSymbols,
        &mut temporal::PlacementHistory,
    ),
) -> Vec<SDFShaderFeatureMetadata> {
    let (boxes, placed, history) = placement;
    let mut metadata = empty_metadata(layer);
    let zoom = view_state.style_zoom().value();
    let uniforms = SymbolUniforms::new(paint, zoom, [1, 1]);
    let mut features: Vec<_> = layer
        .features
        .iter()
        .enumerate()
        .map(|(i, feature)| (i, feature, history.was_visible(layer, feature)))
        .collect();
    features.sort_by(|(_, a, av), (_, b, bv)| {
        a.data
            .sort_key
            .total_cmp(&b.data.sort_key)
            .then_with(|| bv.cmp(av))
    });
    for (feature_index, feature, was_visible) in features {
        let ground = symbol_elevation(world, layer, feature, paint, zoom);
        let relevance = world
            .resources
            .get::<super::visibility::SymbolVisibility>()
            .map_or(1.0, |policy| {
                policy.opacity(layer, feature, ground, view_state)
            });
        let mut limits = zoom_limits;
        if was_visible {
            limits[0] -= 0.15;
            limits[1] += 0.15;
        }
        let rectangles = (relevance > 0.0
            && local_zoom_visible(
                layer.coords,
                feature,
                ground,
                view_state,
                projection,
                limits,
            ))
        .then(|| screen_boxes(layer, feature, ground, view_state, projection, &uniforms))
        .flatten()
        .unwrap_or([None, None]);
        let rules = rules::PlacementRules::new(
            paint,
            &feature.data.properties,
            view_state.style_zoom().value(),
        );
        let visible = rules.place(rectangles, boxes, [view_state.width(), view_state.height()]);
        let opacity = history.opacity(layer, feature, visible);
        if visible.iter().any(|v| *v) && opacity.iter().any(|v| *v > 0.0) {
            placed.0.push(PlacedSymbol {
                coords: layer.coords,
                layer: layer.style_layer_id.clone(),
                feature: feature_index,
                rectangles: [0, 1].map(|i| if visible[i] { rectangles[i] } else { None }),
            });
        }
        write_feature_metadata(
            layer,
            feature,
            opacity.map(|value| value * relevance),
            ground,
            &mut metadata,
        );
    }
    metadata
}

fn write_feature_metadata(
    layer: &crate::sdf::SymbolLayerData,
    feature: &crate::sdf::Feature,
    opacity: [f32; 2],
    ground: f32,
    metadata: &mut [SDFShaderFeatureMetadata],
) {
    for index in feature.indices.clone() {
        let kind = layer
            .new_buffer
            .buffer
            .indices
            .get(index)
            .and_then(|index| layer.new_buffer.buffer.vertices.get(*index as usize))
            .map_or(0, |vertex| usize::from(vertex.a_data[2] != 0));
        if let Some(vertex) = layer
            .new_buffer
            .buffer
            .indices
            .get(index)
            .and_then(|index| metadata.get_mut(*index as usize))
        {
            *vertex = SDFShaderFeatureMetadata {
                opacity: opacity[kind],
                elevation: ground,
            };
        }
    }
}

fn local_zoom_visible(
    coords: WorldTileCoords,
    feature: &crate::sdf::Feature,
    ground: f32,
    view: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    limits: [f64; 2],
) -> bool {
    let Some(clip) = super::placement::project(
        coords,
        [
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        ],
        f64::from(ground),
        view,
        projection,
    ) else {
        return false;
    };
    if clip.w <= 0.0 {
        return false;
    }
    let zoom = view.style_zoom().value()
        + if view.has_external_view() {
            view.symbol_distance_ratio(clip).log2().min(0.0)
        } else {
            0.0
        };
    zoom >= limits[0] && zoom < limits[1]
}

fn allocation(world: &crate::tcs::world::World, coords: WorldTileCoords, id: &str) -> Option<u64> {
    let Initialized(pool) = world.resources.get::<Eventually<SymbolBufferPool>>()? else {
        return None;
    };
    pool.index()
        .get_layers(coords)?
        .iter()
        .find(|entry| entry.style_layer.id == id)
        .map(|entry| entry.allocation_id())
}

impl CollisionSystem {
    fn place_layers(
        &mut self,
        world: &crate::tcs::world::World,
        style: &crate::style::Style,
        view_state: &crate::render::view_state::ViewState,
        projection: &crate::render::projection::ShaderProjectionData,
        queue: &wgpu::Queue,
        layers: Vec<VisibleLayer<'_>>,
    ) -> PlacedSymbols {
        let mut boxes = CollisionGrid::new(view_state.width(), view_state.height());
        let mut updates = Vec::new();
        let mut placed = PlacedSymbols::default();
        for (_, layer, paint) in layers {
            let limits = style
                .layers
                .iter()
                .find(|style| style.id == layer.style_layer_id)
                .map(|layer| {
                    [
                        f64::from(layer.minzoom.unwrap_or(0)),
                        f64::from(layer.maxzoom.unwrap_or(24)),
                    ]
                })
                .unwrap_or([0.0, 24.0]);
            let metadata = place_layer(
                world,
                view_state,
                projection,
                layer,
                paint,
                limits,
                (&mut boxes, &mut placed, &mut self.history),
            );
            let key = (layer.coords, layer.style_layer_id.clone());
            let Some(generation) = allocation(world, layer.coords, &layer.style_layer_id) else {
                continue;
            };
            let fingerprint = opacity_fingerprint(&metadata);
            if self.uploaded.get(&key) != Some(&(generation, fingerprint)) {
                updates.push((key.clone(), metadata));
                self.uploaded.insert(key, (generation, fingerprint));
            }
        }
        if let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() {
            for ((coords, id), metadata) in updates {
                if let Some(entry) = pool
                    .index()
                    .get_layers(coords)
                    .and_then(|entries| entries.iter().find(|entry| entry.style_layer.id == id))
                {
                    pool.update_feature_metadata(queue, entry, &metadata);
                }
            }
        }
        placed
    }
}

type VisibleLayer<'a> = (
    u32,
    &'a crate::sdf::SymbolLayerData,
    &'a crate::style::layer::SymbolPaint,
);

fn empty_metadata(layer: &crate::sdf::SymbolLayerData) -> Vec<SDFShaderFeatureMetadata> {
    vec![
        SDFShaderFeatureMetadata {
            opacity: 0.0,
            elevation: 0.0
        };
        layer.new_buffer.buffer.vertices.len()
    ]
}

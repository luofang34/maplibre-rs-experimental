//! Places elevated text and icons once for both eyes, caching metadata by content and allocation.
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

/// Places visible symbols across tiles and shares placement between eyes in the same frame.
/// Retains fade history and refreshes opacity/elevation uploads when content or allocation changes.
#[derive(Default)]
pub struct CollisionSystem {
    runs: u32,
    /// View the last placement ran for; a different one makes it stale within a frame.
    placed_view: Option<(cgmath::Matrix4<f64>, (f64, f64))>,
    history: temporal::PlacementHistory,
    uploaded: HashMap<(WorldTileCoords, String), (u64, u64)>,
}

impl CollisionSystem {
    /// Starts with no placement history or uploaded metadata fingerprints.
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
        let view = (view_state.view_projection().0, view_state.viewport_size());
        // A head-tracked view changes every frame, so it keeps the periodic cadence instead of
        // placing every label each frame.
        let moved = !view_state.has_external_view()
            && AsRef::<[[f64; 4]; 4]>::as_ref(&view.0)
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            && self.placed_view.as_ref() != Some(&view);
        if !new_content && !moved && !self.history.fading && !self.runs.is_multiple_of(8) {
            return Ok(());
        }
        self.placed_view = Some(view);
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

type OrderedFeature<'a> = (usize, usize, &'a crate::sdf::Feature, bool);

/// Features of all `layers` in placement order: ascending `symbol-sort-key`, equal keys in tile
/// order, and within a tile the labels that were visible first so they keep their place.
fn ordered_features<'a>(
    layers: &[&'a crate::sdf::SymbolLayerData],
    history: &temporal::PlacementHistory,
) -> Vec<OrderedFeature<'a>> {
    let mut features: Vec<_> = layers
        .iter()
        .enumerate()
        .flat_map(|(position, layer)| {
            layer
                .features
                .iter()
                .enumerate()
                .map(move |(i, feature)| (position, i, feature))
        })
        .map(|(position, i, feature)| {
            (
                position,
                i,
                feature,
                history.was_visible(layers[position], feature),
            )
        })
        .collect();
    features.sort_by(|(pa, _, a, av), (pb, _, b, bv)| {
        a.data
            .sort_key
            .total_cmp(&b.data.sort_key)
            .then_with(|| pa.cmp(pb))
            .then_with(|| bv.cmp(av))
    });
    features
}

/// Places the features of one style layer across every visible tile together, in ascending
/// `symbol-sort-key` order, and returns each tile's metadata in the order of `layers`.
fn place_layer(
    world: &crate::tcs::world::World,
    view_state: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    layers: &[&crate::sdf::SymbolLayerData],
    paint: &crate::style::layer::SymbolPaint,
    zoom_limits: [f64; 2],
    placement: (
        &mut CollisionGrid,
        &mut PlacedSymbols,
        &mut temporal::PlacementHistory,
    ),
) -> Vec<Vec<SDFShaderFeatureMetadata>> {
    let (boxes, placed, history) = placement;
    let mut metadata: Vec<_> = layers.iter().map(|layer| empty_metadata(layer)).collect();
    let zoom = view_state.style_zoom().value();
    let uniforms = SymbolUniforms::new(paint, zoom, [1, 1]);
    let features = ordered_features(layers, history);
    for (position, feature_index, feature, was_visible) in features {
        let layer = layers[position];
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
            &mut metadata[position],
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
            .buffer
            .buffer
            .indices
            .get(index)
            .and_then(|index| layer.buffer.buffer.vertices.get(*index as usize))
            .map_or(0, |vertex| usize::from(vertex.a_data[2] != 0));
        if let Some(vertex) = layer
            .buffer
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
        let mut groups: Vec<(
            u32,
            &crate::style::layer::SymbolPaint,
            Vec<&crate::sdf::SymbolLayerData>,
        )> = Vec::new();
        for (index, layer, paint) in layers {
            match groups.last_mut() {
                Some((last, _, members))
                    if *last == index && members[0].style_layer_id == layer.style_layer_id =>
                {
                    members.push(layer);
                }
                _ => groups.push((index, paint, vec![layer])),
            }
        }
        for (_, paint, members) in groups {
            let limits = style
                .layers
                .iter()
                .find(|style| style.id == members[0].style_layer_id)
                .map(|layer| [layer.minzoom.unwrap_or(0.0), layer.maxzoom.unwrap_or(24.0)])
                .unwrap_or([0.0, 24.0]);
            let metadata = place_layer(
                world,
                view_state,
                projection,
                &members,
                paint,
                limits,
                (&mut boxes, &mut placed, &mut self.history),
            );
            for (layer, metadata) in members.into_iter().zip(metadata) {
                let key = (layer.coords, layer.style_layer_id.clone());
                let Some(generation) = allocation(world, layer.coords, &layer.style_layer_id)
                else {
                    continue;
                };
                let fingerprint = opacity_fingerprint(&metadata);
                if self.uploaded.get(&key) != Some(&(generation, fingerprint)) {
                    updates.push((key.clone(), metadata));
                    self.uploaded.insert(key, (generation, fingerprint));
                }
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
        layer.buffer.buffer.vertices.len()
    ]
}

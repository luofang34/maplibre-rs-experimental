//! Places elevated text and icons once for both eyes, caching metadata by content and allocation.
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
};

use layer_pass::place_layer;

#[cfg(test)]
use super::placement::canonical_tile;
use super::{collision_grid::CollisionGrid, query::PlacedSymbols};
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
    draw_sort: draw_sort::DrawSort,
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
        entry.pose.map(f32::to_bits).hash(&mut hash);
        entry.color.map(f32::to_bits).hash(&mut hash);
        entry.halo.map(f32::to_bits).hash(&mut hash);
        entry.params.map(f32::to_bits).hash(&mut hash);
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
        self.draw_sort.follow_bearing(
            world,
            &renderer.queue,
            (
                view_state.camera().get_bearing().0,
                view_state.style_zoom().value(),
            ),
            &layers,
        );
        layers.sort_by_key(|(index, layer, _)| {
            (
                std::cmp::Reverse(*index),
                std::cmp::Reverse(u8::from(layer.coords.z)),
                layer.coords.x,
                layer.coords.y,
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
        world.resources.insert(super::query::PlacementBearing(
            view_state.camera().get_bearing().0,
        ));
        // Labels still fading in or out change the next frame.
        if self.history.fading {
            crate::render::frame_signals::keep_animating(world);
        }
        Ok(())
    }
}

mod draw_sort;
mod family;
mod layer_pass;
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
        queue: &crate::render::upload_queue::UploadQueue,
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
        // A layer GL JS would group with a lower one is placed by it, after the pass.
        let leaders = family::leaders(style);
        let present: HashSet<&str> = groups
            .iter()
            .map(|(_, _, members)| members[0].style_layer_id.as_str())
            .collect();
        let mut outcomes = HashMap::new();
        let mut mirrored = Vec::new();
        let mut placed_layers = Vec::new();
        for (_, paint, members) in groups {
            let id = members[0].style_layer_id.clone();
            if let Some(leader) = leaders
                .get(&id)
                .filter(|leader| **leader != id && present.contains(leader.as_str()))
            {
                mirrored.push((leader.clone(), paint, members));
                continue;
            }
            let limits = style
                .layers
                .iter()
                .find(|style| style.id == id)
                .map(|layer| [layer.minzoom.unwrap_or(0.0), layer.maxzoom.unwrap_or(24.0)])
                .unwrap_or([0.0, 24.0]);
            let (metadata, outcome) = place_layer(
                world,
                view_state,
                projection,
                &members,
                paint,
                limits,
                (&mut boxes, &mut placed, &mut self.history),
            );
            if leaders.get(&id) == Some(&id) {
                outcomes.insert(id, (members.clone(), outcome));
            }
            placed_layers.push((members, metadata));
        }
        for (leader, paint, members) in mirrored {
            let Some((leader_members, outcome)) = outcomes.get(&leader) else {
                continue;
            };
            let metadata = family::mirror(
                (leader_members, outcome),
                &members,
                (paint, view_state.style_zoom().value()),
                &mut placed,
            );
            placed_layers.push((members, metadata));
        }
        for (members, metadata) in placed_layers {
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

//! Keeps symbol tiles until replacement glyphs and GPU geometry are ready.
use std::collections::HashSet;

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::TileKind,
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::ShaderTileMetadata,
        tile_view_pattern::WgpuTileViewPattern,
        view_state::ViewState,
    },
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    style::Style,
    tcs::world::World,
};

pub(crate) struct SymbolCovering {
    pub tiles: Vec<WorldTileCoords>,
    pub buffer: wgpu::Buffer,
}

pub(super) fn targets(world: &World) -> Vec<WorldTileCoords> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    if let Some(Initialized(pattern)) = world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        for tile in pattern.iter() {
            tile.render_kind(TileKind::Vector, |shape| {
                if seen.insert(shape.coords()) {
                    targets.push(shape.coords());
                }
            });
        }
    }
    targets
}

pub(super) fn upload_tiles(world: &World) -> Vec<WorldTileCoords> {
    let mut tiles = world
        .resources
        .get::<SymbolCovering>()
        .map(|covering| covering.tiles.clone())
        .unwrap_or_default();
    let mut seen: HashSet<_> = tiles.iter().copied().collect();
    for target in targets(world) {
        if seen.insert(target) {
            tiles.push(target);
        }
        let mut current = target;
        while let Some(parent) = current.get_parent() {
            current = parent;
            if world
                .tiles
                .query::<&SymbolLayersDataComponent>(parent)
                .is_some_and(|symbols| !symbols.pending_assets)
            {
                if seen.insert(parent) {
                    tiles.push(parent);
                }
                break;
            }
        }
    }
    tiles
}

fn ready(world: &World, style: &Style, zoom: f64, coords: WorldTileCoords) -> bool {
    let Some(symbols) = world.tiles.query::<&SymbolLayersDataComponent>(coords) else {
        return false;
    };
    if symbols.pending_assets {
        return false;
    }
    let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
        return false;
    };
    let loaded = pool.get_loaded_style_layers_at(coords).unwrap_or_default();
    symbols.layers.iter().all(|layer| {
        let visible = style
            .layers
            .iter()
            .any(|style| style.id == layer.style_layer_id && style.is_visible_at(zoom));
        !visible
            || layer.new_buffer.buffer.indices.is_empty()
            || loaded.contains(layer.style_layer_id.as_str())
    })
}

pub(super) fn select(
    targets: &[WorldTileCoords],
    ready: impl Fn(WorldTileCoords) -> bool,
) -> Vec<WorldTileCoords> {
    let mut selected = HashSet::new();
    for &target in targets {
        let mut current = target;
        loop {
            if ready(current) {
                selected.insert(current);
                break;
            }
            let Some(parent) = current.get_parent() else {
                break;
            };
            current = parent;
        }
    }
    // Fine labels keep their placement while a neighbour still needs a parent. Collision
    // priority resolves duplicate anchors across levels without hiding an entire region.
    let mut result: Vec<_> = selected.into_iter().collect();
    result.sort_by_key(|tile| (tile.z, tile.y, tile.x));
    result
}

pub(super) fn update(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    let tiles = if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        world
            .resources
            .get::<SymbolCovering>()
            .map(|covering| covering.tiles.clone())
            .unwrap_or_default()
    } else {
        select(&targets(world), |coords| {
            ready(world, style, view.style_zoom().value(), coords)
        })
    };
    let entries: Vec<_> = tiles.iter().map(|coords| metadata(*coords, view)).collect();
    let bytes = bytemuck::cast_slice(&entries);
    let required = (bytes.len() as u64).max(256);
    let grow = world
        .resources
        .get::<SymbolCovering>()
        .is_none_or(|covering| covering.buffer.size() < required);
    if grow {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("symbol tile transforms"),
            size: required.next_power_of_two(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        world.resources.insert(SymbolCovering {
            tiles: Vec::new(),
            buffer,
        });
    }
    if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
        queue.write_buffer(&covering.buffer, 0, bytes);
        covering.tiles = tiles;
    }
}

fn metadata(coords: WorldTileCoords, view: &ViewState) -> ShaderTileMetadata {
    let zoom = view.zoom();
    let zoom_factor = view.style_zoom().scale_to_tile(&coords) as f32;
    ShaderTileMetadata {
        transform: view
            .gpu_view_projection()
            .to_model_view_projection(coords.transform_for_zoom(zoom))
            .downcast()
            .into(),
        zoom_factor,
        viewport_width: view.width() as f32,
        viewport_height: view.height() as f32,
        tile_mercator_coords: tile_mercator_coordinates(
            coords.into_tile(crate::style::source::TileAddressingScheme::XYZ),
        )
        .into(),
        line_width_scale: 1.0,
        line_units_per_pixel: 8.0 * zoom_factor,
        clip_antimeridian: u32::from(u8::from(coords.z) == 0),
    }
}

#[cfg(test)]
mod tests;

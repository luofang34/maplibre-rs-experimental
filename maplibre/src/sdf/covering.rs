//! Keeps symbol tiles until replacement glyphs and GPU geometry are ready.
use rustc_hash::FxHashSet as HashSet;

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

type SelectionKey = (u64, u64, u64, u64, u64);

#[derive(Clone, Copy, PartialEq, Eq)]
struct CoveringViewKey {
    gpu_vp_bits: [[u64; 4]; 4],
    zoom_bits: u64,
    style_zoom_bits: u64,
    viewport_bits: (u64, u64),
}

impl CoveringViewKey {
    fn of(view: &ViewState) -> Self {
        let m: [[f64; 4]; 4] = view.gpu_view_projection().0.into();
        Self {
            gpu_vp_bits: m.map(|col| col.map(f64::to_bits)),
            zoom_bits: view.zoom().value().to_bits(),
            style_zoom_bits: view.style_zoom().value().to_bits(),
            viewport_bits: (view.width().to_bits(), view.height().to_bits()),
        }
    }
}

pub(crate) struct SymbolCovering {
    pub tiles: Vec<WorldTileCoords>,
    pub buffer: wgpu::Buffer,
    selection_key: Option<SelectionKey>,
    all_targets_ready: bool,
    last_upload_view: Option<CoveringViewKey>,
    metadata_scratch: Vec<ShaderTileMetadata>,
    last_uploaded_bytes: Vec<u8>,
}

impl Drop for SymbolCovering {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub(super) fn targets(world: &World) -> Vec<WorldTileCoords> {
    let mut seen = HashSet::default();
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

fn visible_symbol_hash(style: &Style, zoom: f64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    for (idx, layer) in style.layers.iter().enumerate() {
        if layer.type_ == "symbol" && layer.is_visible_at(zoom) {
            idx.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn visible_symbol_ids(style: &Style, zoom: f64) -> Vec<crate::style::StyleLayerId> {
    style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "symbol" && layer.is_visible_at(zoom))
        .map(crate::style::layer::StyleLayer::layer_id)
        .collect()
}

fn ready(
    world: &World,
    visible_layers: &[crate::style::StyleLayerId],
    coords: WorldTileCoords,
) -> bool {
    let Some(symbols) = world.tiles.query::<&SymbolLayersDataComponent>(coords) else {
        return false;
    };
    if symbols.pending_assets {
        return false;
    }
    let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
        return false;
    };
    let loaded = pool.index().get_layers(coords);
    symbols.layers.iter().all(|layer| {
        let style_id = crate::style::StyleLayerId::intern(&layer.style_layer_id);
        !visible_layers.contains(&style_id)
            || layer.buffer.usable_indices == 0
            || layer.buffer.buffer.indices.is_empty()
            || loaded.is_some_and(|entries| entries.iter().any(|e| e.style_layer_id == style_id))
    })
}

fn select_with_status(
    targets: &[WorldTileCoords],
    ready: impl Fn(WorldTileCoords) -> bool,
) -> (Vec<WorldTileCoords>, bool) {
    let mut selected = HashSet::default();
    let mut all_targets_ready = !targets.is_empty();
    for &target in targets {
        let mut current = target;
        let mut first = true;
        loop {
            if ready(current) {
                selected.insert(current);
                break;
            }
            if first {
                all_targets_ready = false;
                first = false;
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
    (result, all_targets_ready)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn select(
    targets: &[WorldTileCoords],
    ready: impl Fn(WorldTileCoords) -> bool,
) -> Vec<WorldTileCoords> {
    select_with_status(targets, ready).0
}

pub(super) fn update(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
) {
    let key = covering_selection_key(world, style, view);
    let can_reuse = crate::render::eye_covering::EyeInFrame::reuses_content(world)
        || world
            .resources
            .get::<SymbolCovering>()
            .is_some_and(|c| c.all_targets_ready && c.selection_key == Some(key));
    if can_reuse {
        refresh_reused_covering(world, view, queue);
        return;
    }
    let visible_layers = visible_symbol_ids(style, view.style_zoom().value());
    let (tiles, all_targets_ready) = select_with_status(&targets(world), |coords| {
        ready(world, &visible_layers, coords)
    });
    if tiles.is_empty() {
        if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
            covering.tiles.clear();
            covering.selection_key = Some(key);
            covering.all_targets_ready = false;
            covering.last_upload_view = None;
        }
        return;
    }
    upload_covering_tiles(
        world,
        view,
        device,
        queue,
        tiles,
        Some(key),
        all_targets_ready,
    );
}

fn covering_selection_key(world: &World, style: &Style, view: &ViewState) -> SelectionKey {
    let topology_gen = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.topology_generation(),
        _ => 0,
    };
    let (pool_rev, pool_evict) = match world.resources.get::<Eventually<SymbolBufferPool>>() {
        Some(Initialized(pool)) => (pool.revision(), pool.eviction_generation()),
        _ => (0, 0),
    };
    (
        topology_gen,
        pool_rev,
        pool_evict,
        style.revision(),
        visible_symbol_hash(style, view.style_zoom().value()),
    )
}

fn refresh_reused_covering(
    world: &mut World,
    view: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
) {
    let Some(covering) = world.resources.get_mut::<SymbolCovering>() else {
        return;
    };
    if covering.tiles.is_empty() {
        return;
    }
    let view_key = CoveringViewKey::of(view);
    if covering.last_upload_view == Some(view_key) {
        return;
    }
    let SymbolCovering {
        tiles,
        buffer,
        last_upload_view,
        metadata_scratch,
        last_uploaded_bytes,
        ..
    } = covering;
    metadata_scratch.clear();
    metadata_scratch.extend(tiles.iter().map(|coords| metadata(*coords, view)));
    let bytes = bytemuck::cast_slice(metadata_scratch.as_slice());
    if !bytes.is_empty() && last_uploaded_bytes.as_slice() != bytes {
        queue.write_buffer(buffer, 0, bytes);
        last_uploaded_bytes.clear();
        last_uploaded_bytes.extend_from_slice(bytes);
    }
    *last_upload_view = Some(view_key);
}

fn upload_covering_tiles(
    world: &mut World,
    view: &ViewState,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: Vec<WorldTileCoords>,
    selection_key: Option<SelectionKey>,
    all_targets_ready: bool,
) {
    let mut entries = world
        .resources
        .get_mut::<SymbolCovering>()
        .map(|c| std::mem::take(&mut c.metadata_scratch))
        .unwrap_or_default();
    entries.clear();
    entries.extend(tiles.iter().map(|coords| metadata(*coords, view)));
    let bytes = bytemuck::cast_slice(&entries);
    let required = (bytes.len() as u64).max(256);
    if world
        .resources
        .get::<SymbolCovering>()
        .is_none_or(|covering| covering.buffer.size() < required)
    {
        if let Some(old) = world.resources.remove::<SymbolCovering>() {
            queue.cancel_staged_buffer(&old.buffer);
            old.buffer.destroy();
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("symbol tile transforms"),
            size: required.next_power_of_two(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        world.resources.insert(SymbolCovering {
            tiles: Vec::new(),
            buffer,
            selection_key: None,
            all_targets_ready: false,
            last_upload_view: None,
            metadata_scratch: Vec::new(),
            last_uploaded_bytes: Vec::new(),
        });
    }
    let view_key = CoveringViewKey::of(view);
    if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
        if !bytes.is_empty() && covering.last_uploaded_bytes.as_slice() != bytes {
            queue.write_buffer(&covering.buffer, 0, bytes);
            covering.last_uploaded_bytes.clear();
            covering.last_uploaded_bytes.extend_from_slice(bytes);
        }
        covering.tiles = tiles;
        covering.selection_key = selection_key;
        covering.all_targets_ready = all_targets_ready;
        covering.last_upload_view = Some(view_key);
        covering.metadata_scratch = entries;
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
        clip_region: crate::render::shaders::WHOLE_TILE,
        _pad: [0; 2],
    }
}

#[cfg(test)]
mod tests;

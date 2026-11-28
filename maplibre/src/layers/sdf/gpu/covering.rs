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

type SelectionKey = (u64, u64, u64, u64, u64, u64, usize);

#[derive(Clone, Copy, PartialEq, Eq)]
struct CoveringViewKey {
    gpu_vp_bits: [[u64; 4]; 4],
    zoom_bits: u64,
    style_zoom_bits: u64,
    viewport_bits: (u64, u64),
}

impl CoveringViewKey {
    fn of(view: &ViewState, gpu_vp: &crate::render::camera::ViewProjection) -> Self {
        let m: [[f64; 4]; 4] = gpu_vp.0.into();
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
    last_upload_view: Option<CoveringViewKey>,
    metadata_scratch: Vec<ShaderTileMetadata>,
    last_uploaded_bytes: Vec<u8>,
    pub(in crate::layers::sdf) tiles_version: u64,
    pub(in crate::layers::sdf) phase_key: Option<(u64, u64, u64, u64, u64)>,
    pub(in crate::layers::sdf) queue_scratch: Vec<crate::render::render_phase::TranslucentItem>,
    visible_key: Option<(u64, f64, f64)>,
    pub(in crate::layers::sdf) visible_hash: u64,
    visible_ids: Vec<crate::style::StyleLayerId>,
}

impl Drop for SymbolCovering {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub(in crate::layers::sdf) fn targets(world: &World) -> Vec<WorldTileCoords> {
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

pub(in crate::layers::sdf) fn upload_tiles(world: &World) -> Vec<WorldTileCoords> {
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

fn visible_symbols_band(
    style: &Style,
    zoom: f64,
) -> (u64, Vec<crate::style::StyleLayerId>, f64, f64) {
    use std::hash::{Hash, Hasher};
    let indices = style.cached_symbol_layer_indices();
    let mut hasher = rustc_hash::FxHasher::default();
    let mut ids = Vec::with_capacity(indices.len());
    let mut band_lo = f64::NEG_INFINITY;
    let mut band_hi = f64::INFINITY;
    for &idx in indices.iter() {
        let Some(layer) = style.layers.get(idx) else {
            continue;
        };
        if layer.is_hidden() {
            continue;
        }
        if let Some(minzoom) = layer.minzoom
            && minzoom > 0.0
        {
            if zoom >= minzoom {
                band_lo = band_lo.max(minzoom);
            } else {
                band_hi = band_hi.min(minzoom);
            }
        }
        if let Some(maxzoom) = layer.maxzoom {
            if zoom < maxzoom {
                band_hi = band_hi.min(maxzoom);
            } else {
                band_lo = band_lo.max(maxzoom);
            }
        }
        if layer.is_visible_at(zoom) {
            idx.hash(&mut hasher);
            ids.push(layer.layer_id());
        }
    }
    (hasher.finish(), ids, band_lo, band_hi)
}

fn compute_visible_symbols(style: &Style, zoom: f64) -> (u64, Vec<crate::style::StyleLayerId>) {
    let (hash, ids, _, _) = visible_symbols_band(style, zoom);
    (hash, ids)
}

#[inline]
fn visible_key_matches(key: Option<(u64, f64, f64)>, style_rev: u64, zoom: f64) -> bool {
    matches!(key, Some((rev, lo, hi)) if rev == style_rev && zoom >= lo && zoom < hi)
}

fn sync_visible_symbols(world: &mut World, style: &Style, zoom: f64) -> u64 {
    let style_rev = style.revision();
    if let Some(covering) = world.resources.get::<SymbolCovering>()
        && visible_key_matches(covering.visible_key, style_rev, zoom)
    {
        return covering.visible_hash;
    }
    let (hash, ids, band_lo, band_hi) = visible_symbols_band(style, zoom);
    if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
        covering.visible_key = Some((style_rev, band_lo, band_hi));
        covering.visible_hash = hash;
        covering.visible_ids = ids;
    }
    hash
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
    let loaded = pool.as_ref().get_layers(coords);
    symbols.layers.iter().all(|layer| {
        let style_id = crate::style::StyleLayerId::intern(&layer.style_layer_id);
        !visible_layers.contains(&style_id)
            || layer.buffer.usable_indices == 0
            || layer.buffer.indices.is_empty()
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
pub(in crate::layers::sdf) fn select(
    targets: &[WorldTileCoords],
    ready: impl Fn(WorldTileCoords) -> bool,
) -> Vec<WorldTileCoords> {
    select_with_status(targets, ready).0
}

pub(in crate::layers::sdf) fn update(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
) {
    let style_zoom = view.style_zoom().value();
    let vis_hash = sync_visible_symbols(world, style, style_zoom);
    let key = covering_selection_key(world, style, vis_hash);
    let can_reuse = crate::render::eye_covering::EyeInFrame::reuses_content(world)
        || world
            .resources
            .get::<SymbolCovering>()
            .is_some_and(|c| c.selection_key == Some(key));
    if can_reuse {
        refresh_reused_covering(world, style, view, queue);
        return;
    }
    let visible_layers = world
        .resources
        .get_mut::<SymbolCovering>()
        .filter(|c| visible_key_matches(c.visible_key, style.revision(), style_zoom))
        .map_or_else(
            || compute_visible_symbols(style, style_zoom).1,
            |c| std::mem::take(&mut c.visible_ids),
        );
    let (tiles, _) = select_with_status(&targets(world), |coords| {
        ready(world, &visible_layers, coords)
    });
    if let Some(covering) = world.resources.get_mut::<SymbolCovering>()
        && covering.visible_ids.is_empty()
    {
        covering.visible_ids = visible_layers;
    }
    if tiles.is_empty() {
        if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
            if !covering.tiles.is_empty() {
                covering.tiles.clear();
                covering.tiles_version = covering.tiles_version.wrapping_add(1);
                covering.phase_key = None;
            }
            covering.selection_key = Some(key);
            covering.last_upload_view = None;
        }
        return;
    }
    upload_covering_tiles(world, style, view, device, queue, tiles, Some(key));
}

fn covering_selection_key(world: &World, style: &Style, visible_hash: u64) -> SelectionKey {
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
        visible_hash,
        crate::tcs::tile_epochs::vector_arrival_epoch(world),
        world.tiles.len(),
    )
}

pub(in crate::layers::sdf) fn frame_gpu_view_projection(
    world: &World,
    style: &Style,
    view: &ViewState,
) -> crate::render::camera::ViewProjection {
    world
        .resources
        .get::<crate::render::resolved_view::ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view))
        .map_or_else(
            || view.gpu_view_projection(),
            |resolved| resolved.gpu_view_projection,
        )
}

fn refresh_reused_covering(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
) {
    let gpu_vp = frame_gpu_view_projection(world, style, view);
    let Some(covering) = world.resources.get_mut::<SymbolCovering>() else {
        return;
    };
    if covering.tiles.is_empty() {
        return;
    }
    let view_key = CoveringViewKey::of(view, &gpu_vp);
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
    metadata_scratch.extend(tiles.iter().map(|coords| metadata(*coords, view, &gpu_vp)));
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
    style: &Style,
    view: &ViewState,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    tiles: Vec<WorldTileCoords>,
    selection_key: Option<SelectionKey>,
) {
    let gpu_vp = frame_gpu_view_projection(world, style, view);
    let mut entries = world
        .resources
        .get_mut::<SymbolCovering>()
        .map(|c| std::mem::take(&mut c.metadata_scratch))
        .unwrap_or_default();
    entries.clear();
    entries.extend(tiles.iter().map(|coords| metadata(*coords, view, &gpu_vp)));
    let bytes = bytemuck::cast_slice(&entries);
    let required = (bytes.len() as u64).max(256);
    if world
        .resources
        .get::<SymbolCovering>()
        .is_none_or(|covering| covering.buffer.size() < required)
    {
        let (prev_vis_key, prev_vis_hash, prev_vis_ids, prev_tiles_ver, prev_queue_scratch) =
            world.resources.remove::<SymbolCovering>().map_or_else(
                || (None, 0, Vec::new(), 0, Vec::new()),
                |mut old| {
                    queue.cancel_staged_buffer(&old.buffer);
                    old.buffer.destroy();
                    (
                        old.visible_key,
                        old.visible_hash,
                        std::mem::take(&mut old.visible_ids),
                        old.tiles_version,
                        std::mem::take(&mut old.queue_scratch),
                    )
                },
            );
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
            last_upload_view: None,
            metadata_scratch: Vec::new(),
            last_uploaded_bytes: Vec::new(),
            tiles_version: prev_tiles_ver,
            phase_key: None,
            queue_scratch: prev_queue_scratch,
            visible_key: prev_vis_key,
            visible_hash: prev_vis_hash,
            visible_ids: prev_vis_ids,
        });
    }
    let view_key = CoveringViewKey::of(view, &gpu_vp);
    if let Some(covering) = world.resources.get_mut::<SymbolCovering>() {
        if !bytes.is_empty() && covering.last_uploaded_bytes.as_slice() != bytes {
            queue.write_buffer(&covering.buffer, 0, bytes);
            covering.last_uploaded_bytes.clear();
            covering.last_uploaded_bytes.extend_from_slice(bytes);
        }
        if covering.tiles != tiles {
            covering.tiles = tiles;
            covering.tiles_version = covering.tiles_version.wrapping_add(1);
            covering.phase_key = None;
        }
        covering.selection_key = selection_key;
        covering.last_upload_view = Some(view_key);
        covering.metadata_scratch = entries;
    }
}

fn metadata(
    coords: WorldTileCoords,
    view: &ViewState,
    gpu_vp: &crate::render::camera::ViewProjection,
) -> ShaderTileMetadata {
    let zoom = view.zoom();
    let zoom_factor = view.style_zoom().scale_to_zoom_level(coords.z) as f32;
    ShaderTileMetadata {
        transform: gpu_vp
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

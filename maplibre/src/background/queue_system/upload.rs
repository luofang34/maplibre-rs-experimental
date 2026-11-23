//! Layer-queueing and GPU buffer upload helpers for background, sky, and atmosphere draws.

use super::super::{
    buffers::{background_color, BackgroundBuffers, BackgroundGpuBuffers},
    render_commands::{
        DrawAtmosphere, DrawBackground, DrawBackgroundPattern, DrawGlobeBackground,
        DrawGlobeBackgroundPattern, DrawSky,
    },
};
use crate::{
    coords::WorldTileCoords,
    render::{
        camera::ViewProjection,
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TranslucentItem},
        shaders::{AtmosphereLayerMetadata, BackgroundLayerMetadata, SkyLayerMetadata},
        view_state::ViewState,
        Renderer,
    },
    style::Style,
    tcs::{system::SystemError, world::World},
};

pub(super) struct BackgroundFrameParams {
    pub frame: [f32; 2],
    pub horizon: [f32; 4],
    pub world_rows: [[f32; 4]; 2],
    pub projection_transition: f32,
    pub atmosphere_blend: f32,
    pub resolved_vp: Option<ViewProjection>,
}

pub(super) fn enqueue_background_layers(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    params: &BackgroundFrameParams,
    has_sky: bool,
) -> Result<(bool, bool, Option<TranslucentItem>), SystemError> {
    let Some((layer_item_phase, gpu_buffers)) = world
        .resources
        .query_mut::<(&mut RenderPhase<LayerItem>, &mut BackgroundGpuBuffers)>()
    else {
        return Err(SystemError::Dependencies);
    };
    let scratch = &mut gpu_buffers.scratch;
    scratch.metadatas.clear();
    scratch.instances.clear();
    scratch.opaque_layers.clear();
    let indices_len = scratch.bg_layer_indices(style).len();
    let mut background_index = 0;
    let mut has_patterned_bg = false;
    for pos in 0..indices_len {
        let Some(layer) = scratch
            .bg_layer_indices(style)
            .get(pos)
            .and_then(|&idx| style.layers.get(idx))
        else {
            continue;
        };
        if push_background_layer(layer, style, view_state, params, scratch, layer_item_phase) {
            has_patterned_bg = true;
            background_index = background_index.max(layer.index);
        } else if scratch.instances.contains_key(&layer.layer_id()) {
            background_index = background_index.max(layer.index);
        }
    }
    let pending_atmosphere = queue_sky_and_atmosphere(
        layer_item_phase,
        background_index,
        has_sky,
        params.atmosphere_blend,
    );
    let has_draws = !scratch.metadatas.is_empty() || params.atmosphere_blend > 0.0 || has_sky;
    Ok((has_draws, has_patterned_bg, pending_atmosphere))
}

fn push_background_layer(
    layer: &crate::style::layer::StyleLayer,
    style: &Style,
    view_state: &ViewState,
    params: &BackgroundFrameParams,
    scratch: &mut super::super::buffers::BackgroundScratch,
    layer_item_phase: &mut RenderPhase<LayerItem>,
) -> bool {
    let uses_globe = params.projection_transition > 0.0;
    let zoom = view_state.zoom().value();
    let style_zoom = view_state.style_zoom().value();
    let has_terrain = style.terrain.is_some();
    if !layer.is_visible_at(zoom) || (has_terrain && !uses_globe) {
        return false;
    }
    if crate::vector::pattern::names_missing_image(layer.paint.as_ref(), style, style_zoom) {
        return false;
    }
    let is_patterned = layer
        .paint
        .as_ref()
        .is_some_and(|paint| crate::vector::pattern::pattern_name(paint, style_zoom).is_some());
    let c = background_color(layer, zoom);
    if uses_globe && !is_patterned && c[3] <= 0.0 {
        return false;
    }
    let layer_id = layer.layer_id();
    if !is_patterned && c[3] >= 1.0 {
        scratch.opaque_layers.insert(layer_id);
    }
    scratch
        .instances
        .insert(layer_id, scratch.metadatas.len() as u32);
    scratch.metadatas.push(BackgroundLayerMetadata {
        color: c,
        z_index: layer.index as f32,
        padding: [0.0; 3],
        horizon: params.horizon,
        viewport: [
            view_state.height() as f32,
            f32::from(has_terrain),
            params.frame[0],
            params.frame[1],
        ],
        world_rows: params.world_rows,
    });
    let draw_function: &'static dyn crate::render::render_phase::Draw<LayerItem> =
        match (uses_globe, is_patterned) {
            (true, true) => &DrawState::<LayerItem, DrawGlobeBackgroundPattern>::INSTANCE,
            (true, false) => &DrawState::<LayerItem, DrawGlobeBackground>::INSTANCE,
            (false, true) => &DrawState::<LayerItem, DrawBackgroundPattern>::INSTANCE,
            (false, false) => &DrawState::<LayerItem, DrawBackground>::INSTANCE,
        };
    layer_item_phase.add(LayerItem {
        projection: ProjectionBinding::View,
        draw_function,
        index: layer.index,
        generate_borders: false,
        style_layer: layer_id,
        entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
        source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
        mask_group: crate::render::render_phase::MaskGroup::Unmasked,
        tile: crate::tcs::tiles::Tile {
            coords: WorldTileCoords::default(),
        },
        run: None,
    });
    is_patterned
}

fn queue_sky_and_atmosphere(
    layer_item_phase: &mut RenderPhase<LayerItem>,
    background_index: u32,
    has_sky: bool,
    atmosphere_blend: f32,
) -> Option<TranslucentItem> {
    if has_sky {
        layer_item_phase.add(LayerItem {
            projection: ProjectionBinding::View,
            draw_function: &DrawState::<LayerItem, DrawSky>::INSTANCE,
            index: background_index,
            generate_borders: false,
            style_layer: crate::style::StyleLayerId::SKY,
            entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
            source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
            mask_group: crate::render::render_phase::MaskGroup::Unmasked,
            tile: crate::tcs::tiles::Tile {
                coords: WorldTileCoords::default(),
            },
            run: None,
        });
    }
    (atmosphere_blend > 0.0).then_some(TranslucentItem {
        draw_function: &DrawState::<TranslucentItem, DrawAtmosphere>::INSTANCE,
        index: u32::MAX,
        style_layer: crate::style::StyleLayerId::ATMOSPHERE,
        entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
        tile: crate::tcs::tiles::Tile {
            coords: WorldTileCoords::default(),
        },
        source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
    })
}

fn stage_background_buffers(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    renderer: &Renderer,
    params: &BackgroundFrameParams,
    (sky_metadata, atmosphere_metadata): (
        Option<&SkyLayerMetadata>,
        Option<&AtmosphereLayerMetadata>,
    ),
) -> u64 {
    let pure_globe = params.projection_transition >= 1.0;
    let gpu_vp = (!pure_globe).then(|| {
        params
            .resolved_vp
            .unwrap_or_else(|| view_state.gpu_view_projection())
    });
    let gpu_buffers = world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
    let alloc_gen = gpu_buffers.allocation_generation();
    let need_tile_write = {
        let scratch = &mut gpu_buffers.scratch;
        if scratch.metadatas.is_empty() {
            scratch.metadatas.push(BackgroundLayerMetadata {
                color: [0.0; 4],
                z_index: 0.0,
                padding: [0.0; 3],
                horizon: params.horizon,
                viewport: [
                    view_state.height() as f32,
                    f32::from(style.terrain.is_some()),
                    params.frame[0],
                    params.frame[1],
                ],
                world_rows: params.world_rows,
            });
        }
        scratch.prepare_tile_metadata(view_state, gpu_vp.as_ref(), pure_globe, alloc_gen)
    };
    let _ = gpu_buffers.write_staged_metadata(&renderer.device, &renderer.queue);
    if need_tile_write {
        let _ = gpu_buffers.write_staged_tile_metadata(&renderer.device, &renderer.queue);
    }
    if let Some(atmo) = atmosphere_metadata {
        let _ = gpu_buffers.write_atmosphere(&renderer.device, &renderer.queue, atmo);
    } else if gpu_buffers.atmosphere_metadata_buffer().is_none() {
        let disabled = AtmosphereLayerMetadata::disabled();
        let _ = gpu_buffers.write_atmosphere(&renderer.device, &renderer.queue, &disabled);
    }
    if let Some(sky) = sky_metadata {
        let _ = gpu_buffers.write_sky(&renderer.device, &renderer.queue, sky);
    }
    gpu_buffers.allocation_generation()
}

pub(super) fn upload_and_publish_background(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    renderer: &Renderer,
    params: &BackgroundFrameParams,
    (sky_metadata, atmosphere_metadata): (
        Option<SkyLayerMetadata>,
        Option<AtmosphereLayerMetadata>,
    ),
) {
    let uses_globe = params.projection_transition > 0.0;
    let globe_tiles_changed = if uses_globe {
        world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
        if let Some((gpu_buffers, shared)) = world.resources.query_mut::<(
            &mut BackgroundGpuBuffers,
            &crate::render::eye_covering::SharedCovering,
        )>() {
            gpu_buffers.scratch.sync_globe_tiles(shared.tiles())
        } else {
            world
                .resources
                .get_or_init_mut::<BackgroundGpuBuffers>()
                .scratch
                .sync_globe_tiles(None)
        }
    } else {
        world
            .resources
            .get_or_init_mut::<BackgroundGpuBuffers>()
            .scratch
            .clear_globe_tiles()
    };
    let flat_covers_viewport = !uses_globe
        && super::flat_viewport_covered(view_state, style.terrain.is_none(), params.frame);
    let buf_gen = stage_background_buffers(
        world,
        style,
        view_state,
        renderer,
        params,
        (sky_metadata.as_ref(), atmosphere_metadata.as_ref()),
    );
    publish_background_buffers(
        world,
        renderer,
        (buf_gen, flat_covers_viewport, globe_tiles_changed),
        (sky_metadata.as_ref(), atmosphere_metadata.as_ref()),
    );
}

fn publish_background_buffers(
    world: &mut World,
    renderer: &Renderer,
    (buf_gen, flat_covers_viewport, globe_tiles_changed): (u64, bool, bool),
    (sky_metadata, atmosphere_metadata): (
        Option<&SkyLayerMetadata>,
        Option<&AtmosphereLayerMetadata>,
    ),
) {
    let Some((gpu_buffers, existing)) = world
        .resources
        .query_mut::<(&mut BackgroundGpuBuffers, &mut BackgroundBuffers)>()
    else {
        insert_background_buffers(
            world,
            renderer,
            flat_covers_viewport,
            (sky_metadata, atmosphere_metadata),
        );
        return;
    };
    if existing.buffer_generation != buf_gen {
        existing.metadata_buffer = gpu_buffers.metadata_buffer().cloned().unwrap_or_else(|| {
            gpu_buffers
                .write_staged_metadata(&renderer.device, &renderer.queue)
                .clone()
        });
        let (globe_layer_bind_group, globe_layer_stride) = gpu_buffers
            .globe_layer_binding()
            .map(|(bg, stride)| (bg.clone(), stride))
            .unwrap_or_else(|| {
                gpu_buffers.write_staged_globe_layers(&renderer.device, &renderer.queue)
            });
        existing.globe_layer_bind_group = globe_layer_bind_group;
        existing.globe_layer_stride = globe_layer_stride;
        existing.tile_metadata_buffer =
            gpu_buffers
                .tile_metadata_buffer()
                .cloned()
                .unwrap_or_else(|| {
                    gpu_buffers
                        .write_staged_tile_metadata(&renderer.device, &renderer.queue)
                        .clone()
                });
        existing.atmosphere_metadata_buffer = gpu_buffers
            .atmosphere_metadata_buffer()
            .cloned()
            .unwrap_or_else(|| {
                let disabled = AtmosphereLayerMetadata::disabled();
                gpu_buffers
                    .write_atmosphere(&renderer.device, &renderer.queue, &disabled)
                    .clone()
            });
        existing.sky_metadata_buffer =
            sky_metadata.and_then(|_| gpu_buffers.sky_metadata_buffer().cloned());
        existing.buffer_generation = buf_gen;
    } else if sky_metadata.is_some() {
        if existing.sky_metadata_buffer.is_none() {
            existing.sky_metadata_buffer = gpu_buffers.sky_metadata_buffer().cloned();
        }
    } else {
        existing.sky_metadata_buffer = None;
    }
    existing.flat_covers_viewport = flat_covers_viewport;
    std::mem::swap(&mut existing.instances, &mut gpu_buffers.scratch.instances);
    std::mem::swap(
        &mut existing.opaque_layers,
        &mut gpu_buffers.scratch.opaque_layers,
    );
    if globe_tiles_changed || existing.globe_tiles.len() != gpu_buffers.scratch.globe_tiles.len() {
        existing
            .globe_tiles
            .clone_from(&gpu_buffers.scratch.globe_tiles);
    }
}

fn insert_background_buffers(
    world: &mut World,
    renderer: &Renderer,
    flat_covers_viewport: bool,
    (sky_metadata, atmosphere_metadata): (
        Option<&SkyLayerMetadata>,
        Option<&AtmosphereLayerMetadata>,
    ),
) {
    let gpu_buffers = world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
    let metadata_buffer = gpu_buffers
        .write_staged_metadata(&renderer.device, &renderer.queue)
        .clone();
    let (globe_layer_bind_group, globe_layer_stride) =
        gpu_buffers.write_staged_globe_layers(&renderer.device, &renderer.queue);
    let tile_metadata_buffer = gpu_buffers
        .write_staged_tile_metadata(&renderer.device, &renderer.queue)
        .clone();
    let disabled = AtmosphereLayerMetadata::disabled();
    let atmo_ref = atmosphere_metadata.unwrap_or(&disabled);
    let atmosphere_metadata_buffer = gpu_buffers
        .write_atmosphere(&renderer.device, &renderer.queue, atmo_ref)
        .clone();
    let sky_metadata_buffer = sky_metadata.map(|sky| {
        gpu_buffers
            .write_sky(&renderer.device, &renderer.queue, sky)
            .clone()
    });
    let instances = std::mem::take(&mut gpu_buffers.scratch.instances);
    let opaque_layers = std::mem::take(&mut gpu_buffers.scratch.opaque_layers);
    let globe_tiles = gpu_buffers.scratch.globe_tiles.clone();
    let buf_gen = gpu_buffers.allocation_generation();
    world.resources.insert(BackgroundBuffers {
        metadata_buffer,
        globe_layer_bind_group,
        globe_layer_stride,
        instances,
        tile_metadata_buffer,
        globe_tiles,
        atmosphere_metadata_buffer,
        sky_metadata_buffer,
        flat_covers_viewport,
        opaque_layers,
        buffer_generation: buf_gen,
    });
}

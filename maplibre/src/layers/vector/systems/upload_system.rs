//! Uploads data to the GPU which is needed for rendering.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::FrameUploads,
        Renderer,
    },
    schedule::system::{SystemError, SystemResult},
    style::Style,
    vector::VectorBufferPool,
};

mod bucket;
#[cfg(feature = "geojson")]
pub(crate) mod dynamic_points;
mod frame_state;
mod metadata;
pub(crate) mod paint;
mod shared_fallback;
pub(crate) mod sources;
pub(crate) mod style_refresh;
mod tile_pass;

use bucket::SharedStyleLayers;
use frame_state::frame_paint;
pub(crate) use frame_state::VectorPaintFrame;
#[cfg(test)]
use metadata::{metadata_for_layer, pattern_runs, sort_runs};
#[cfg(all(test, feature = "headless"))]
pub(crate) use tile_pass::upload_tessellated_layer;
use tile_pass::{
    upload_tessellated_layer_with_uncovered, TessellatedUploadPlan, TessellatedUploadState,
};

#[cfg(feature = "geojson")]
pub(crate) type DynPoints = dynamic_points::DynamicPointGeoJson;
#[cfg(not(feature = "geojson"))]
#[derive(Default)]
pub(crate) struct DynPoints;

#[inline]
fn layer_is_dynamic_circle(style: &Style, style_layer: &crate::style::layer::StyleLayer) -> bool {
    #[cfg(feature = "geojson")]
    {
        dynamic_points::uses_dynamic_circle_metadata(style, style_layer)
    }
    #[cfg(not(feature = "geojson"))]
    {
        let _ = (style, style_layer);
        false
    }
}

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer {
            queue, settings, ..
        },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let eviction_gen = match world.resources.get::<Eventually<VectorBufferPool>>() {
        None => return Ok(()),
        Some(Initialized(pool)) => pool.eviction_generation(),
        Some(_) => return Err(SystemError::Dependencies),
    };
    if settings.buffer_pools.is_empty()
        || crate::render::eye_covering::EyeInFrame::reuses_content(world)
    {
        super::request_system::clear_upload_permits(world);
        return Ok(());
    }
    let (paint_frame, previous_paint) = frame_paint(world, style, view_state);
    let refresh_paint = previous_paint != Some(paint_frame);
    let upload_sources = sources::frame_sources_and_painted_mut(world, style, view_state)?;
    init_upload_resources(world);
    let has_pending_uploads =
        check_pending_vector_uploads(world, style, eviction_gen, &upload_sources.source_tiles)?;
    let permit_source_set = super::request_system::snapshot_permit_sources(
        world,
        has_pending_uploads,
        &upload_sources.source_tiles,
    );
    let terrain_booting = style.terrain.is_some()
        && world
            .resources
            .get::<crate::terrain::request_system::DrapeRequests>()
            .is_none_or(|requests| requests.0.is_empty());
    if has_pending_uploads && !terrain_booting {
        if let Some(evictor) = world.resources.get::<crate::render::TileEvictor>().copied() {
            (evictor.0)(
                world,
                style,
                view_state,
                queue,
                settings.tight_tile_requests,
                &upload_sources.painted_tiles,
            );
        }
    }
    let (spatial, changed) =
        crate::vector::structures::for_frame(world, style, &upload_sources.painted_tiles);
    #[cfg(feature = "geojson")]
    dynamic_points::ensure_seeded_for_style(world, style);
    let dirty_style_applied = style_refresh::apply_dirty_style(world, style, queue, paint_frame);
    let retain_loaded_tiles = world
        .resources
        .get::<crate::render::RetainLoadedTiles>()
        .is_some();
    let Some((Initialized(buffer_pool), budget, dyn_points, paint_scratch, shared_layers)) =
        world.resources.query_mut::<(
            &mut Eventually<VectorBufferPool>,
            &mut FrameUploads,
            &mut DynPoints,
            &mut paint::PaintRefreshScratch,
            &mut SharedStyleLayers,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };
    let moving = budget.is_moving();
    let (run_paint_refresh, tiles_changed) = paint_scratch.prepare_refresh(
        style,
        &upload_sources.active_painted_tiles,
        buffer_pool.eviction_generation(),
        (dirty_style_applied, refresh_paint, moving),
    );
    if run_paint_refresh {
        paint_scratch.refresh_layer_paint(
            buffer_pool,
            queue,
            paint::PaintRefreshContext {
                tiles: &world.tiles,
                source_tiles: &upload_sources.active_painted_tiles,
                frame: paint_frame,
                previous: previous_paint,
                moving,
                tiles_changed,
            },
        );
    }
    let paint_settle_pending = paint_scratch.needs_settle();
    if changed {
        crate::vector::structures::refresh_gpu(buffer_pool, queue, &spatial, &world.tiles);
    }
    let (deferred, needs_refetch) = if has_pending_uploads {
        let allow_cpu_release = (style.terrain.is_none() || cfg!(target_arch = "wasm32"))
            && !retain_loaded_tiles
            && settings.buffer_pools.vertices >= 1_000_000;
        let shared_style_layers = shared_layers.sync(style);
        upload_tessellated_layer_with_uncovered(
            buffer_pool,
            queue,
            &mut world.tiles,
            TessellatedUploadPlan {
                style,
                shared_style_layers,
                source_tiles: &upload_sources.source_tiles,
                uncovered_sources: &upload_sources.uncovered,
                spatial: &spatial,
                paint: paint_frame,
                allow_cpu_release,
            },
            TessellatedUploadState {
                budget,
                dyn_points,
                paint_scratch: Some(paint_scratch),
            },
        )
    } else {
        (false, false)
    };
    if has_pending_uploads {
        crate::vector::bump_upload_stamp_epoch(world);
    }
    #[cfg(feature = "geojson")]
    dynamic_points::refresh_dynamic_point_layers(
        world,
        style,
        queue,
        &upload_sources.active_painted_tiles,
    );
    if deferred || paint_settle_pending {
        crate::render::frame_signals::keep_animating(world);
    }
    super::request_system::reconcile_upload_permits(
        world,
        deferred,
        style.revision(),
        permit_source_set.as_ref(),
    );
    if needs_refetch {
        crate::render::frame_signals::mark_dirty(world);
    }
    Ok(())
}

fn check_pending_vector_uploads(
    world: &mut crate::tcs::world::World,
    style: &Style,
    eviction_gen: u64,
    source_tiles: &std::sync::Arc<[crate::coords::WorldTileCoords]>,
) -> Result<bool, SystemError> {
    let tiles = &world.tiles;
    let stamp = (style.revision(), eviction_gen);
    let epochs = (
        crate::vector::arrival_epoch(world),
        crate::vector::upload_stamp_epoch(world),
    );
    Ok(world
        .resources
        .get_mut::<paint::PaintRefreshScratch>()
        .is_none_or(|scratch| scratch.check_pending_uploads(tiles, stamp, epochs, source_tiles)))
}

fn init_upload_resources(world: &mut crate::tcs::world::World) {
    world.resources.get_or_init_mut::<FrameUploads>();
    world.resources.get_or_init_mut::<DynPoints>();
    world
        .resources
        .get_or_init_mut::<paint::PaintRefreshScratch>();
    world.resources.get_or_init_mut::<SharedStyleLayers>();
}

#[cfg(test)]
mod tests;

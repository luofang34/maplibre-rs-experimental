//! Uploads data to the GPU which is needed for rendering.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::FrameUploads,
        Renderer,
    },
    style::Style,
    tcs::system::{SystemError, SystemResult},
    vector::{VectorBufferPool, VectorLayerBucketComponent},
};

mod bucket;
mod frame_state;
mod metadata;
pub(crate) mod paint;
mod shared_fallback;
pub(crate) mod sources;
pub(crate) mod style_refresh;
mod tile_pass;

use bucket::SharedStyleLayers;
pub(crate) use frame_state::drape_paint_zoom;
use frame_state::{frame_paint, VectorPaintFrame};
#[cfg(test)]
use metadata::{metadata_for_layer, pattern_runs, sort_runs};
#[cfg(all(test, feature = "headless"))]
use tile_pass::upload_tessellated_layer;
use tile_pass::{
    upload_tessellated_layer_with_uncovered, TessellatedUploadPlan, TessellatedUploadState,
};

#[cfg(feature = "geojson")]
type DynPoints = crate::geojson::update::dynamic_points::DynamicPointGeoJson;
#[cfg(not(feature = "geojson"))]
#[derive(Default)]
struct DynPoints;

#[inline]
fn layer_is_dynamic_circle(style: &Style, style_layer: &crate::style::layer::StyleLayer) -> bool {
    #[cfg(feature = "geojson")]
    {
        crate::geojson::update::dynamic_points::uses_dynamic_circle_metadata(style, style_layer)
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
    let Some(pool_state) = world.resources.get::<Eventually<VectorBufferPool>>() else {
        return Ok(());
    };
    if !matches!(pool_state, Initialized(_)) {
        return Err(SystemError::Dependencies);
    }
    if settings.buffer_pools.is_empty()
        || crate::render::eye_covering::EyeInFrame::reuses_content(world)
    {
        super::request_system::clear_upload_permits(world);
        return Ok(());
    }
    let (paint_frame, previous_paint) = frame_paint(world, style, view_state);
    let refresh_paint = previous_paint != Some(paint_frame);
    let (source_tiles, painted_tiles, uncovered_sources) =
        sources::frame_sources_and_painted_mut(world, style, view_state)?;
    let has_pending_uploads = has_pending_vector_uploads(world, style, &source_tiles)?;
    let permit_source_set =
        super::request_system::snapshot_permit_sources(world, has_pending_uploads, &source_tiles);
    let terrain_booting = style.terrain.is_some()
        && world
            .resources
            .get::<crate::terrain::request_system::DrapeRequests>()
            .is_none_or(|requests| requests.0.is_empty());
    if has_pending_uploads && !terrain_booting {
        crate::render::systems::retention_system::evict_before_upload(
            world,
            style,
            view_state,
            queue,
            settings.tight_tile_requests,
            &painted_tiles,
        );
    }
    let (spatial, changed) = super::structures::for_frame(world, style, &painted_tiles);
    init_upload_resources(world);
    #[cfg(feature = "geojson")]
    crate::geojson::update::dynamic_points::ensure_seeded_for_style(world, style);
    let dirty_style_applied = style_refresh::apply_dirty_style(world, style, queue, paint_frame);
    let retain_loaded_tiles = world
        .resources
        .get::<crate::render::systems::retention_system::RetainLoadedTiles>()
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
    if dirty_style_applied {
        paint_scratch.slots.invalidate_all();
        paint_scratch.invalidate_tiles();
    }
    paint_scratch
        .slots
        .sync_generations(style.revision(), buffer_pool.eviction_generation());
    let moving = budget.is_moving();
    let (run_paint_refresh, tiles_changed) = paint_scratch.slots.check_refresh(
        &painted_tiles,
        refresh_paint,
        !moving && paint_scratch.needs_settle(),
    );
    if run_paint_refresh {
        paint_scratch.sync_style(style);
        paint::refresh_layer_paint(
            buffer_pool,
            queue,
            paint::PaintRefreshContext {
                tiles: &world.tiles,
                source_tiles: &painted_tiles,
                frame: paint_frame,
                previous: previous_paint,
                moving,
                tiles_changed,
            },
            paint_scratch,
        );
    }
    let paint_settle_pending = paint_scratch.needs_settle();
    if changed {
        super::structures::refresh_gpu(buffer_pool, queue, &spatial, &world.tiles);
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
                source_tiles: &source_tiles,
                uncovered_sources: &uncovered_sources,
                spatial: &spatial,
                paint: paint_frame,
                allow_cpu_release,
            },
            TessellatedUploadState {
                budget,
                dyn_points,
                paint_slots: Some(&mut paint_scratch.slots),
            },
        )
    } else {
        (false, false)
    };
    #[cfg(feature = "geojson")]
    crate::geojson::update::dynamic_points::refresh_dynamic_point_layers(
        world,
        style,
        queue,
        &painted_tiles,
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

fn has_pending_vector_uploads(
    world: &crate::tcs::world::World,
    style: &Style,
    source_tiles: &[crate::coords::WorldTileCoords],
) -> Result<bool, SystemError> {
    let stamp = match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => (style.revision(), pool.eviction_generation()),
        _ => return Err(SystemError::Dependencies),
    };
    Ok(!source_tiles.iter().all(|&coords| {
        world
            .tiles
            .query::<(
                &VectorLayerBucketComponent,
                &super::content::LayerReplacements,
            )>(coords)
            .is_some_and(|(layers, pending)| {
                layers.done && pending.vector.is_empty() && layers.uploaded_stamp == Some(stamp)
            })
    }))
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

#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
#[path = "upload_system/retry/tests.rs"]
mod retry_tests;

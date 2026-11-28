//! Map-level orchestration plugin for 3D terrain rendering and multi-layer texture draping.

use std::{marker::PhantomData, rc::Rc};

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{draw_graph, eventually::Eventually},
    schedule::{system::SystemContainer, RenderStageLabel, Schedule},
    tcs::world::World,
    terrain::{
        coverage, drape_timing, elevation, interaction, populate_world_system::PopulateWorldSystem,
        request_system::RequestSystem, resource_system, resources::TerrainResources,
        transferables::DemTransferables, upload_system, DrapePassNode, DrapePhase,
        TerrainCoverageIndex, TerrainFrame, DRAPE_PASS,
    },
};

mod drape_phase;
pub(crate) mod drape_prepare;
mod frame;
mod layer_specs;
mod polar;
mod prepare_uniforms;
mod queue;
mod readiness;
mod spec_cache;
pub(crate) mod targets;
#[cfg(test)]
mod tests;

/// Requests, stores, drapes and draws the DEM tiles of a style that declares `terrain`.
pub struct TerrainPlugin<T>(PhantomData<T>);

fn resolve_terrain_frame_view_system(
    ctx: &mut crate::context::MapContext,
) -> crate::schedule::system::SystemResult {
    if ctx.style.terrain.is_none() {
        return Ok(());
    }
    super::runtime::frame::resolve_frame_view_system(ctx)
}

/// Lifts the camera's orbit point onto the terrain under the map center every frame, records
/// the lowest elevation of the tile under it for the far plane, and enforces eye/near-plane
/// terrain clearance for constrained map cameras on boot, async DEM load, and jumps.
pub fn center_elevation_system(
    crate::context::MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut crate::context::MapContext,
) -> crate::schedule::system::SystemResult {
    if style.terrain.is_none() {
        return Ok(());
    }
    let (center_ready, vp_before) = elevation::step_center_elevation(view_state, world);
    let lifted = center_ready
        && view_state.camera_authority()
            == crate::render::view_state::CameraAuthority::ConstrainedMap
        && interaction::keep_camera_above_terrain(style, view_state, world);
    elevation::finish_center_elevation(style, view_state, world, vp_before, lifted);
    Ok(())
}

impl<T: DemTransferables> Default for TerrainPlugin<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E: Environment, T: DemTransferables> Plugin<E> for TerrainPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        world
            .resources
            .insert(Eventually::<TerrainResources>::Uninitialized);
        world.resources.init::<DrapePhase>();
        world.resources.init::<TerrainFrame>();
        world.resources.init::<TerrainCoverageIndex>();
        coverage::install_elevation_bridge(world);
        crate::render::pass_refs::PassRefPopulators::register(
            world,
            crate::terrain::populate_terrain_pass_refs,
        );

        schedule.add_early_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        )?;
        // Sync DEM bounds first so center elevation sees arriving DEMs, then resolve center
        // elevation before building the tight terrain covering and `PreparedTerrainRegion`.
        schedule
            .add_system_to_stage(RenderStageLabel::Prepare, coverage::sync_dem_bounds_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Prepare, center_elevation_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Prepare, coverage::coverage_system)?;
        schedule
            .add_system_to_stage(RenderStageLabel::Prepare, resolve_terrain_frame_view_system)?;
        schedule
            .add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue::queue_system)?;
        // Before the Render stage, so the last measurement is read before this frame's drape
        // passes start the next one.
        schedule.add_system_to_stage(
            RenderStageLabel::PhaseSort,
            drape_timing::drape_timer_system,
        )?;
        world
            .resources
            .get_or_init_mut::<crate::render::BeforeSubmitHooks>()
            .register(drape_timing::resolve_before_submit);
        world
            .resources
            .get_or_init_mut::<crate::render::AfterSubmitCallbacks>()
            .register(drape_timing::map_after_submit);

        let draw_graph = &mut renderer.render_graph;
        let edges = [
            draw_graph.add_node(DRAPE_PASS, DrapePassNode),
            draw_graph.add_node_edge(DRAPE_PASS, draw_graph::node::MAIN_PASS),
        ];
        for edge in edges {
            edge?;
        }
        Ok(())
    }
}

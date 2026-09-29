//! Removes uploaded data or constrains metadata between upload and terrain queueing.

use std::rc::Rc;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    headless::environment::HeadlessEnvironment,
    kernel::Kernel,
    plugin::Plugin,
    raster::resource::RasterResources,
    render::{
        eventually::Eventually, graph::RenderGraph, resource::BackingBufferDescriptor,
        shaders::ShaderTileMetadata, tile_view_pattern::WgpuTileViewPattern, RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::SystemResult, world::World},
};

#[derive(Default)]
pub(super) struct Faults {
    pub(super) missing_upload: Option<WorldTileCoords>,
    pub(super) metadata_capacity: Option<usize>,
    pub(super) applied: bool,
    saved_pattern: Option<WgpuTileViewPattern>,
}

pub(super) struct FaultPlugin;

impl Plugin<HeadlessEnvironment> for FaultPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<HeadlessEnvironment>>,
        world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        world.resources.insert(Faults::default());
        schedule.add_system_to_stage(RenderStageLabel::Queue, inject);
        schedule.add_system_to_stage(RenderStageLabel::Cleanup, restore);
    }
}

fn inject(context: &mut MapContext) -> SystemResult {
    let resources = &mut context.world.resources;
    let mut faults = std::mem::take(resources.get_mut::<Faults>().expect("fault controls"));
    if let Some(coords) = faults.missing_upload {
        let Some(Eventually::Initialized(raster)) =
            resources.get_mut::<Eventually<RasterResources>>()
        else {
            panic!("raster resources");
        };
        assert!(
            raster.get_bound_texture(&coords).is_some(),
            "evict a real upload"
        );
        raster.remove_texture(coords);
        faults.applied = true;
    }
    if let Some(capacity) = faults.metadata_capacity {
        let Some(Eventually::Initialized(pattern)) =
            resources.get_mut::<Eventually<WgpuTileViewPattern>>()
        else {
            panic!("tile pattern");
        };
        let limited = WgpuTileViewPattern::new(BackingBufferDescriptor::new(
            pattern.buffer().clone(),
            (capacity * std::mem::size_of::<ShaderTileMetadata>()) as u64,
        ));
        faults.saved_pattern = Some(std::mem::replace(pattern, limited));
        faults.applied = true;
    }
    resources.insert(faults);
    Ok(())
}

fn restore(context: &mut MapContext) -> SystemResult {
    let resources = &mut context.world.resources;
    if let Some(pattern) = resources
        .get_mut::<Faults>()
        .expect("fault controls")
        .saved_pattern
        .take()
    {
        resources.insert(Eventually::Initialized(pattern));
    }
    Ok(())
}

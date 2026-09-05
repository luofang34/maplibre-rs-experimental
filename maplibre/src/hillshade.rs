//! Hillshade and colour relief layers: DEM tiles shaded on the GPU.
//!
//! Both layer types read their raster-dem source through the raster tile path: the tiles are
//! fetched, decoded and uploaded like imagery, and the shaders decode elevations from the
//! texels. Hillshade takes the slope from the eight neighbours of a texel and shades it with
//! the layer's lights; colour relief maps the elevation through the layer's colour ramp. With
//! terrain both still draw to the screen rather than into the drape textures.

use std::rc::Rc;

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{eventually::Eventually, graph::RenderGraph, RenderStageLabel},
    schedule::Schedule,
    tcs::world::World,
};

mod prepare_system;
mod queue_system;
pub mod render_commands;
mod resource_system;
pub mod resources;

pub use resources::{DemLayerKind, HillshadeResources};

/// Registers the DEM-shaded layer types; the raster plugin must be registered as well, since
/// it fetches and uploads the tiles.
#[derive(Default)]
pub struct HillshadePlugin;

impl<E: Environment> Plugin<E> for HillshadePlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<E>>,
        world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<HillshadeResources>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, prepare_system::prepare_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system);
    }
}

/// Whether a style layer is one of the DEM-shaded kinds.
pub fn dem_layer_kind(layer_type: &str) -> Option<DemLayerKind> {
    match layer_type {
        "hillshade" => Some(DemLayerKind::Hillshade),
        "color-relief" => Some(DemLayerKind::ColorRelief),
        _ => None,
    }
}

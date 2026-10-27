//! The moon's disc, lit on the side the sun direction shows it.
//!
//! The phase is geometry only: each pixel of the disc stands for a point on a sphere, lit where
//! its normal faces the sun direction, so the lit share and the bright limb's orientation both
//! follow from the sun and moon directions. The dark side keeps a faint earthshine. With no sun
//! direction set the whole disc is drawn lit, a fallback rather than a phase. The disc lies at
//! the far plane like the sun's, and does not fade with the sky: the moon shows by day.

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    celestial::{discs, sun_direction},
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        tracked_pass::TrackedRenderPass,
    },
    tcs::{system::SystemResult, world::World},
};

mod pipeline;

pub use pipeline::{resource_system, MoonPipeline};

/// What the moon's shaders take for the frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MoonFrame {
    /// Direction to the moon's centre in globe world axes, then the tangent of its radius.
    direction_radius: [f32; 4],
    /// Direction to the sun in globe world axes, then 1 when a sun lights the phase.
    sun: [f32; 4],
    /// The globe transition the moon fades in with, then padding.
    transition: [f32; 4],
}

/// The moon's frame values on the GPU, and whether a moon is drawn this frame.
#[derive(Default)]
pub struct MoonGpu {
    frame: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    drawn: bool,
}

impl MoonGpu {
    /// The frame's bind group, when a moon is drawn this frame.
    pub fn drawable(&self) -> Option<&wgpu::BindGroup> {
        let (_, frame) = self.frame.as_ref()?;
        self.drawn.then_some(frame)
    }
}

/// Uploads the moon's frame values, when the host has set a moon and the map is a globe.
pub(super) fn queue_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    let Some(Initialized(MoonPipeline(pipeline))) =
        world.resources.get::<Eventually<MoonPipeline>>()
    else {
        return Ok(());
    };
    let frame_layout = pipeline.get_bind_group_layout(1);
    let sun = sun_direction(world).map_or([0.0; 4], |sun| {
        let sun = sun.globe_world();
        [sun.x as f32, sun.y as f32, sun.z as f32, 1.0]
    });
    let frame = discs(world).moon.map(|moon| {
        let direction = moon.direction().globe_world();
        MoonFrame {
            direction_radius: [
                direction.x as f32,
                direction.y as f32,
                direction.z as f32,
                moon.radius().degrees().to_radians().tan() as f32,
            ],
            sun,
            transition: [transition, 0.0, 0.0, 0.0],
        }
    });
    let gpu = world.resources.get_or_init_mut::<MoonGpu>();
    gpu.drawn = frame.is_some() && transition > 0.0;
    let Some(frame) = frame else {
        return Ok(());
    };
    let (buffer, _) = gpu.frame.get_or_insert_with(|| {
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("moon frame"),
            size: std::mem::size_of::<MoonFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("moon frame"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
        (buffer, bind_group)
    });
    renderer
        .queue
        .write_buffer(buffer, 0, bytemuck::bytes_of(&frame));
    Ok(())
}

/// Draws the moon, if one is set; the translucent pass calls it with the sun, before its items.
pub fn draw<'w>(pass: &mut TrackedRenderPass<'w>, world: &'w World) {
    let Some((Initialized(MoonPipeline(pipeline)), Initialized(projection), gpu)) =
        world.resources.query::<(
            &Eventually<MoonPipeline>,
            &Eventually<ProjectionGpuResources>,
            &MoonGpu,
        )>()
    else {
        return;
    };
    let Some(frame) = gpu.drawable() else {
        return;
    };
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, projection.bind_group(), &[]);
    pass.set_bind_group(1, frame, &[]);
    pass.draw(0..6, 0..1);
}

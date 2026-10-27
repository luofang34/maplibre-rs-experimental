//! The sun's disc and its glare.
//!
//! The disc is limb-darkened and lies at the far plane, tested against depth without writing
//! it, so the ground, terrain and every layer cover the part of it below their horizon. The
//! glare around it brightens with the share of the disc the ground leaves showing: the vertex
//! shader samples the depth the main pass left, copied by [`depth::SunDepthCopyNode`], across
//! the disc. Neither fades with the sky the stars fade with: the sun is what lights it.

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    celestial::discs,
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::{globe_camera_for_view, ProjectionGpuResources},
        tracked_pass::TrackedRenderPass,
    },
    tcs::{system::SystemResult, world::World},
};

pub mod depth;
mod pipeline;

pub use pipeline::{resource_system, SunPipeline};

/// The glare reaches this many disc radii from the sun's centre.
const GLARE_EXTENT: f32 = 12.0;

/// What the sun's shaders take for the frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SunFrame {
    /// Direction to the sun's centre in globe world axes, then its radius in physical pixels.
    direction_radius: [f32; 4],
    /// The viewport in physical pixels, the glare's reach in disc radii, and the globe
    /// transition the sun fades in with.
    viewport_glare_transition: [f32; 4],
}

/// The sun's frame values on the GPU, and whether a sun is drawn this frame.
#[derive(Default)]
pub struct SunGpu {
    frame: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    drawn: bool,
}

impl SunGpu {
    /// The frame's bind group, when a sun is drawn this frame.
    pub fn drawable(&self) -> Option<&wgpu::BindGroup> {
        let (_, frame) = self.frame.as_ref()?;
        self.drawn.then_some(frame)
    }
}

/// The sun's radius in physical pixels: the projection's focal length in pixels times the
/// tangent of its angular radius.
pub fn radius_pixels(projection_y_scale: f64, height: f64, angular_radius_degrees: f64) -> f64 {
    projection_y_scale * height / 2.0 * angular_radius_degrees.to_radians().tan()
}

/// Uploads the sun's frame values, when the host has set a sun disc and the map is a globe.
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
    let Some(Initialized(SunPipeline(pipeline))) = world.resources.get::<Eventually<SunPipeline>>()
    else {
        return Ok(());
    };
    let frame_layout = pipeline.get_bind_group_layout(1);
    let size = renderer.state().surface().size();
    let frame = discs(world)
        .sun
        .zip(globe_camera_for_view(view_state).ok())
        .map(|(sun, camera)| {
            let direction = sun.direction().globe_world();
            let radius = radius_pixels(
                camera.projection().y.y,
                f64::from(size.height()),
                sun.angular_radius_degrees(),
            );
            SunFrame {
                direction_radius: [
                    direction.x as f32,
                    direction.y as f32,
                    direction.z as f32,
                    radius as f32,
                ],
                viewport_glare_transition: [
                    size.width() as f32,
                    size.height() as f32,
                    GLARE_EXTENT,
                    transition,
                ],
            }
        });
    let gpu = world.resources.get_or_init_mut::<SunGpu>();
    gpu.drawn = frame.is_some() && transition > 0.0;
    let Some(frame) = frame else {
        return Ok(());
    };
    let (buffer, _) = gpu.frame.get_or_insert_with(|| {
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun frame"),
            size: std::mem::size_of::<SunFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sun frame"),
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

/// Draws the sun and its glare, if one is set; the translucent pass calls it before its items,
/// so the atmosphere's glow lies over it.
pub fn draw<'w>(pass: &mut TrackedRenderPass<'w>, world: &'w World) {
    let Some((
        Initialized(SunPipeline(pipeline)),
        Initialized(projection),
        gpu,
        Initialized(depth),
    )) = world.resources.query::<(
        &Eventually<SunPipeline>,
        &Eventually<ProjectionGpuResources>,
        &SunGpu,
        &Eventually<depth::SunDepth>,
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
    pass.set_bind_group(2, &depth.binding, &[]);
    pass.draw(0..6, 0..1);
}

#[cfg(test)]
mod tests;

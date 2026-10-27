//! The moon's disc, lit on the side the sun direction shows it, and its halo.
//!
//! The phase is geometry only: each pixel of the disc stands for a point on a sphere, lit where
//! its normal faces the sun direction, so the lit share and the bright limb's orientation both
//! follow from the sun and moon directions. The surface scatters as the moon's regolith does,
//! so a full moon is as bright at its limb as at its centre, and saturates towards white well
//! above any star. A halo around it falls with the square of the angle, as bright as the
//! moon's magnitude at its phase. The dark side keeps a faint earthshine. With no sun
//! direction set the whole disc is drawn lit, a fallback rather than a phase. The disc lies at
//! the far plane like the sun's, and does not fade with the sky: the moon shows by day.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::{InnerSpace, Vector3};

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

/// The halo reaches this many disc radii from the moon's centre.
const HALO_REACH: f32 = 16.0;
/// A full moon's visual magnitude.
const FULL_MOON_MAGNITUDE: f64 = -12.73;

/// The moon's visual magnitude at phase angle `degrees`, the sun-moon-observer angle, as
/// Allen's Astrophysical Quantities fits it: a full moon at zero, fainter towards new.
pub fn magnitude_at_phase_angle(degrees: f64) -> f64 {
    FULL_MOON_MAGNITUDE + 0.026 * degrees.abs() + 4e-9 * degrees.powi(4)
}

/// The moon's brightness relative to a full moon, 10^(-0.4 Δm), from the phase angle the sun
/// and moon directions give, both in the same axes. With no sun the moon is drawn full.
pub fn relative_brightness(sun: Option<Vector3<f64>>, moon: Vector3<f64>) -> f64 {
    let Some(sun) = sun else {
        return 1.0;
    };
    // The sun is far enough that the phase angle is the angle from the moon's direction to the
    // sun's opposite.
    let phase_angle = (-sun.normalize().dot(moon.normalize()))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    10_f64.powf(-0.4 * (magnitude_at_phase_angle(phase_angle) - FULL_MOON_MAGNITUDE))
}

/// What the moon's shaders take for the frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MoonFrame {
    /// Direction to the moon's centre in globe world axes, then the tangent of its radius.
    direction_radius: [f32; 4],
    /// Direction to the sun in globe world axes, then 1 when a sun lights the phase.
    sun: [f32; 4],
    /// The globe transition the moon fades in with, its brightness relative to a full moon,
    /// and the halo's reach in disc radii.
    transition_flux_reach: [f32; 4],
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
    let sun_globe = sun_direction(world).map(|sun| sun.globe_world());
    let sun = sun_globe.map_or([0.0; 4], |sun| {
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
            transition_flux_reach: [
                transition,
                relative_brightness(sun_globe, direction) as f32,
                HALO_REACH,
                0.0,
            ],
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

#[cfg(test)]
mod tests;

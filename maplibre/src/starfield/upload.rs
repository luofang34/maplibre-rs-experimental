//! The stars' GPU copy, rebuilt when the host changes them, and the frame's values.

use bytemuck_derive::{Pod, Zeroable};

use super::StarfieldPipeline;
use crate::{
    celestial::{Star, Starfield},
    context::MapContext,
    render::eventually::{Eventually, Eventually::Initialized},
    tcs::system::SystemResult,
};

/// The magnitude drawn at full brightness with the reference radius.
const REFERENCE_MAGNITUDE: f32 = 1.0;
/// Radius in logical pixels of a star of the reference magnitude is this plus one.
const RADIUS_SCALE: f32 = 1.5;
const MIN_RADIUS: f32 = 1.0;
const MAX_RADIUS: f32 = 4.0;

/// One star as the shader draws it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct StarInstance {
    /// Direction in globe world axes, then the radius in logical pixels.
    pub direction_radius: [f32; 4],
    /// sRGB colour, then the brightness in 0..=1.
    pub color_brightness: [f32; 4],
}

/// How a star is drawn: its flux relative to the reference magnitude, 10^(-0.4 Δm), sets both
/// its brightness, which saturates at the reference, and its size, which keeps growing for the
/// brightest stars until it is capped.
pub fn star_instance(star: &Star) -> StarInstance {
    let direction = star.direction().globe_world();
    let flux = 10_f32.powf(-0.4 * (star.magnitude() - REFERENCE_MAGNITUDE));
    let amplitude = flux.sqrt();
    let radius = (MIN_RADIUS + RADIUS_SCALE * amplitude).clamp(MIN_RADIUS, MAX_RADIUS);
    let [red, green, blue] = star.color();
    StarInstance {
        direction_radius: [
            direction.x as f32,
            direction.y as f32,
            direction.z as f32,
            radius,
        ],
        color_brightness: [red, green, blue, amplitude.min(1.0)],
    }
}

/// The stars on the GPU and the frame's viewport, pixel ratio and globe transition.
#[derive(Default)]
pub struct StarfieldGpu {
    instances: Option<wgpu::Buffer>,
    count: u32,
    generation: Option<u64>,
    frame: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    visible: bool,
}

impl StarfieldGpu {
    /// The instance buffer and frame bind group, when there are stars to draw this frame.
    pub fn drawable(&self) -> Option<(&wgpu::Buffer, &wgpu::BindGroup)> {
        let instances = self.instances.as_ref()?;
        let (_, frame) = self.frame.as_ref()?;
        (self.visible && self.count > 0).then_some((instances, frame))
    }

    /// Number of stars uploaded.
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// Uploads the stars when they changed and the frame's values; stars show only on a globe.
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
    let Some(Initialized(StarfieldPipeline(pipeline))) =
        world.resources.get::<Eventually<StarfieldPipeline>>()
    else {
        return Ok(());
    };
    let frame_layout = pipeline.get_bind_group_layout(1);
    let starfield = world.resources.get::<Starfield>().cloned();
    let pixel_ratio = world
        .resources
        .get::<crate::sdf::provided::DisplayPixelRatio>()
        .map_or(1.0, |ratio| ratio.0);
    let size = renderer.state().surface().size();
    let gpu = world.resources.get_or_init_mut::<StarfieldGpu>();
    gpu.visible = transition > 0.0;
    if let Some(starfield) = starfield.filter(|field| Some(field.generation()) != gpu.generation) {
        let instances: Vec<StarInstance> = starfield.stars().iter().map(star_instance).collect();
        gpu.count = u32::try_from(instances.len()).unwrap_or(u32::MAX);
        gpu.instances = (!instances.is_empty()).then(|| {
            renderer.queue.create_buffer_init(
                &renderer.device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("starfield instances"),
                    contents: bytemuck::cast_slice(&instances),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            )
        });
        gpu.generation = Some(starfield.generation());
    }
    let (buffer, _) = gpu.frame.get_or_insert_with(|| {
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("starfield frame"),
            size: std::mem::size_of::<[f32; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("starfield frame"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
        (buffer, bind_group)
    });
    let frame = [
        size.width() as f32,
        size.height() as f32,
        pixel_ratio,
        transition,
    ];
    renderer
        .queue
        .write_buffer(buffer, 0, bytemuck::cast_slice(&frame));
    Ok(())
}

//! The stars' GPU copy, rebuilt when the host changes them, and the frame's values.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::InnerSpace;

use super::StarfieldPipeline;
use crate::{
    celestial::{Star, Starfield},
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::AtmosphereLayerMetadata,
    },
    tcs::system::SystemResult,
};

/// The magnitude drawn at full brightness with the reference radius.
const REFERENCE_MAGNITUDE: f32 = 1.0;
/// Radius in logical pixels of a star of the reference magnitude is this plus one.
const RADIUS_SCALE: f32 = 1.5;
const MIN_RADIUS: f32 = 1.0;
const MAX_RADIUS: f32 = 4.0;
/// The brightness a star saturates at: short of white, so the moon outshines every star.
pub const STAR_PEAK_BRIGHTNESS: f32 = 0.75;

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
/// its brightness, which saturates at the reference at [`STAR_PEAK_BRIGHTNESS`], and its size,
/// which keeps growing for the brightest stars until it is capped.
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
        color_brightness: [red, green, blue, amplitude.min(1.0) * STAR_PEAK_BRIGHTNESS],
    }
}

/// What the star shader needs of the frame: where the view looks and the sky it looks through.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct StarfieldFrame {
    /// The viewport in physical pixels, physical pixels per logical one, and the globe transition.
    viewport_ratio_transition: [f32; 4],
    /// Columns of the rotation from globe world axes into view axes.
    view_rotation: [[f32; 4]; 3],
    /// The atmosphere's inputs, as [`AtmosphereLayerMetadata::sky_inputs`] gives them.
    sky: [[f32; 4]; 3],
    /// Direction to the moon in globe world axes, then its brightness relative to a full moon;
    /// all zero without a moon.
    moon: [f32; 4],
}

/// The frame's view rotation and sky, with no sky where the atmosphere is not drawn or the
/// globe camera cannot be built.
fn view_and_sky(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> ([[f32; 4]; 3], [[f32; 4]; 3]) {
    let disabled = AtmosphereLayerMetadata::disabled().sky_inputs();
    let Ok(camera) = crate::render::projection::globe_camera_for_view(view_state) else {
        return (
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            disabled,
        );
    };
    let view = camera.view();
    let column = |axis: cgmath::Vector4<f64>| {
        let axis = axis.truncate().normalize();
        [axis.x as f32, axis.y as f32, axis.z as f32, 0.0]
    };
    let rotation = [column(view.x), column(view.y), column(view.z)];
    let blend = crate::background::queue_system::atmosphere_blend(style, view_state, world);
    if blend <= 0.0 {
        return (rotation, disabled);
    }
    let light = style.light.clone().unwrap_or_default();
    let lighting = crate::celestial::lighting(world);
    let sky = AtmosphereLayerMetadata::from_view(
        &camera,
        &light,
        lighting,
        view_state.zoom().value(),
        blend,
    )
    .map_or(disabled, |metadata| metadata.sky_inputs());
    (rotation, sky)
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
    let (view_rotation, sky) = view_and_sky(style, view_state, world);
    let discs = crate::celestial::discs(world);
    let moon = discs.moon.map_or([0.0; 4], |moon| {
        let direction = moon.direction().globe_world();
        let sun = crate::celestial::sun_direction(world).map(|sun| sun.globe_world());
        let brightness = super::moon::eclipsed_brightness(sun, moon, discs.earth_shadow);
        [
            direction.x as f32,
            direction.y as f32,
            direction.z as f32,
            brightness as f32,
        ]
    });
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
            size: std::mem::size_of::<StarfieldFrame>() as u64,
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
    let frame = StarfieldFrame {
        viewport_ratio_transition: [
            size.width() as f32,
            size.height() as f32,
            pixel_ratio,
            transition,
        ],
        view_rotation,
        sky,
        moon,
    };
    renderer
        .queue
        .write_buffer(buffer, 0, bytemuck::bytes_of(&frame));
    Ok(())
}

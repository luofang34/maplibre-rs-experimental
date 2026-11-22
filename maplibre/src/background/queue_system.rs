//! Evaluates visible background paint and queues sky and atmosphere draws.

mod upload;

use cgmath::InnerSpace;
use upload::{enqueue_background_layers, upload_and_publish_background, BackgroundFrameParams};

pub(crate) use super::buffers::globe_tiles;
pub use super::buffers::{BackgroundBuffers, BackgroundGpuBuffers};
use super::{
    buffers::{sky_metadata_with_camera, world_rows},
    globe_sky::{draws_globe_sky, globe_sky_metadata, globe_sky_metadata_with_camera},
};
use crate::{
    context::MapContext,
    projection::globe::camera::GlobeCameraState,
    render::{
        render_phase::{LayerItem, RenderPhase, TranslucentItem},
        resolved_view::ResolvedFrameView,
        shaders::{AtmosphereLayerMetadata, SkyLayerMetadata},
        view_state::ViewState,
    },
    style::light::LightSpecification,
    tcs::system::{SystemError, SystemResult},
};

/// Returns `true` when an unclipped flat quad is guaranteed to pass every `background.fragment.wgsl` check.
pub(crate) fn flat_viewport_covered(view_state: &ViewState, edges: bool, frame: [f32; 2]) -> bool {
    if !edges || frame[0] <= 0.0 || frame[1] <= 0.0 {
        return false;
    }
    let horizon = view_state.horizon_line().to_shader();
    let [row_num, row_den] = world_rows(view_state, edges);
    let logical_h = view_state.height() as f32;
    let scale = logical_h / frame[1].max(1.0);
    for (px, py, nx, ny) in [
        (0.0_f32, 0.0_f32, -1.0_f32, 1.0_f32),
        (frame[0], 0.0, 1.0, 1.0),
        (0.0, frame[1], -1.0, -1.0),
        (frame[0], frame[1], 1.0, -1.0),
    ] {
        let x = px * scale;
        let y = logical_h - py * scale;
        let distance = (y - horizon[1]) * horizon[3] + (x - horizon[0]) * horizon[2];
        if !distance.is_finite() || distance > -1e-3 {
            return false;
        }
        let denom = row_den[0] * nx + row_den[1] * ny + row_den[2];
        if !denom.is_finite() || denom <= 1e-9 {
            return false;
        }
        let row = (row_num[0] * nx + row_num[1] * ny + row_num[2]) / denom;
        if !row.is_finite() || row < row_num[3] + 1e-3 || row > row_den[3] - 1e-3 {
            return false;
        }
    }
    true
}

const SKY_SPACE_CUTOFF_RATIO: f64 = 10.0 * (100_000.0 / 6_371_008.8);

fn atmosphere_blend_state_with(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
    transition: f32,
    resolved_camera: Option<&GlobeCameraState>,
) -> (f32, bool) {
    let zoom = view_state.zoom().value();
    let sky_blend = style
        .sky
        .as_ref()
        .map_or(0.0, |sky| sky.atmosphere_blend_at_zoom(zoom));
    let host_blend = f32::from(u8::from(
        world
            .resources
            .get::<crate::render::SceneLighting>()
            .is_some_and(|l| l.atmosphere),
    ));
    let sky_faded = if sky_blend > 0.0 && transition > 0.0 {
        let owned = if resolved_camera.is_none() {
            ResolvedFrameView::globe_camera_for_frame(world, style, view_state).ok()
        } else {
            None
        };
        let altitude_blend = resolved_camera
            .or(owned.as_deref())
            .map_or(0.0, super::altitude::altitude_blend_for_camera);
        sky_blend * altitude_blend
    } else {
        0.0
    };
    let over_globe = sky_faded > 0.0 && host_blend <= 0.0;
    (sky_faded.max(host_blend) * transition, over_globe)
}

fn atmosphere_blend_state(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> (f32, bool) {
    let zoom = view_state.zoom().value();
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification.projection_type.globe_transition(zoom)
    });
    let resolved_camera = world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view_state))
        .and_then(ResolvedFrameView::globe_camera);
    atmosphere_blend_state_with(style, view_state, world, transition, resolved_camera)
}

/// How strongly the atmosphere is drawn this frame: as the style's `sky` blends it, or in full
/// where the host turned it on, faded with the projection's globe transition and camera altitude below 100 km.
pub fn atmosphere_blend(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> f32 {
    atmosphere_blend_state(style, view_state, world).0
}

fn resolve_sky_and_atmosphere(
    world: &crate::tcs::world::World,
    style: &crate::style::Style,
    view_state: &ViewState,
    resolved_camera: Option<&GlobeCameraState>,
    (frame_height, projection_transition, atmosphere_blend, atmosphere_over_globe, globe_sky): (
        f32,
        f32,
        f32,
        bool,
        bool,
    ),
) -> Result<(Option<SkyLayerMetadata>, Option<AtmosphereLayerMetadata>), SystemError> {
    let external_or_free = view_state.has_external_view()
        || view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe;
    let may_draw_sky = style.sky.is_some()
        && (!external_or_free
            || projection_transition < 1.0
            || view_state.opaque_environment()
            || globe_sky);
    let owned_camera = if may_draw_sky || atmosphere_blend > 0.0 {
        match resolved_camera {
            Some(_) => None,
            None => crate::render::projection::globe_camera_for_view(view_state).ok(),
        }
    } else {
        None
    };
    let cached_globe_camera = resolved_camera.or(owned_camera.as_ref());
    let space_sky_culled = projection_transition >= 1.0
        && !globe_sky
        && cached_globe_camera.is_some_and(|camera| {
            camera.globe_radius_pixels() > 0.0
                && camera.camera_position().magnitude() - 1.0 >= SKY_SPACE_CUTOFF_RATIO
        });
    let sky_metadata = if may_draw_sky && !space_sky_culled {
        style.sky.as_ref().map(|sky| {
            globe_sky
                .then(|| match cached_globe_camera {
                    Some(camera) => globe_sky_metadata_with_camera(sky, view_state, camera),
                    None => globe_sky_metadata(sky, view_state),
                })
                .flatten()
                .unwrap_or_else(|| {
                    sky_metadata_with_camera(
                        sky,
                        view_state,
                        projection_transition,
                        frame_height,
                        cached_globe_camera,
                    )
                })
        })
    } else {
        None
    };
    let atmosphere_metadata = build_atmosphere_metadata(
        world,
        style,
        view_state,
        cached_globe_camera,
        (atmosphere_blend, atmosphere_over_globe),
    )?;
    Ok((sky_metadata, atmosphere_metadata))
}

fn build_atmosphere_metadata(
    world: &crate::tcs::world::World,
    style: &crate::style::Style,
    view_state: &ViewState,
    cached_globe_camera: Option<&GlobeCameraState>,
    (atmosphere_blend, atmosphere_over_globe): (f32, bool),
) -> Result<Option<AtmosphereLayerMetadata>, SystemError> {
    if atmosphere_blend <= 0.0 {
        return Ok(None);
    }
    let Some(camera) = cached_globe_camera else {
        return Err(SystemError::Setup);
    };
    let default_light = LightSpecification::default();
    let light = style.light.as_ref().unwrap_or(&default_light);
    let scene_lighting = world
        .resources
        .get::<crate::render::SceneLighting>()
        .copied()
        .unwrap_or_default();
    AtmosphereLayerMetadata::from_view(
        camera,
        light,
        scene_lighting,
        view_state.zoom().value(),
        atmosphere_blend,
    )
    .map(|metadata| Some(metadata.with_depth_z(if atmosphere_over_globe { 1.0 } else { 0.0 })))
    .map_err(|error| {
        tracing::error!(?error, "failed to build atmosphere shader metadata");
        SystemError::Setup
    })
}

/// Appends visible background, sky and atmosphere items and uploads their frame metadata.
/// Returns `Dependencies` if the required phases are absent, or `Setup` if atmosphere
/// camera or shader metadata cannot be constructed. Existing phase items are not cleared.
pub fn queue_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world.resources.get::<RenderPhase<LayerItem>>().is_none()
        || world
            .resources
            .get::<RenderPhase<TranslucentItem>>()
            .is_none()
    {
        return Err(SystemError::Dependencies);
    }
    let frame = {
        let size = renderer.state().surface().size();
        [size.width() as f32, size.height() as f32]
    };
    let resolved_view = world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view_state));
    let projection_transition = resolved_view.map_or_else(
        || {
            style.projection.as_ref().map_or(0.0, |specification| {
                specification
                    .projection_type
                    .globe_transition(view_state.zoom().value())
            })
        },
        ResolvedFrameView::globe_transition,
    );
    let uses_globe = projection_transition > 0.0;
    let resolved_vp = resolved_view.map(|resolved| resolved.gpu_view_projection);
    let resolved_camera = resolved_view.and_then(ResolvedFrameView::globe_camera);
    let (atmosphere_blend, atmosphere_over_globe) = atmosphere_blend_state_with(
        style,
        view_state,
        world,
        projection_transition,
        resolved_camera,
    );
    let globe_sky = draws_globe_sky(view_state, projection_transition, atmosphere_blend);
    let (sky_metadata, atmosphere_metadata) = resolve_sky_and_atmosphere(
        world,
        style,
        view_state,
        resolved_camera,
        (
            frame[1],
            projection_transition,
            atmosphere_blend,
            atmosphere_over_globe,
            globe_sky,
        ),
    )?;
    let (world_rows, horizon) = if uses_globe {
        (world_rows(view_state, false), [0.0; 4])
    } else {
        (
            world_rows(view_state, style.terrain.is_none()),
            view_state.horizon_line().to_shader(),
        )
    };
    world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
    let params = BackgroundFrameParams {
        frame,
        horizon,
        world_rows,
        projection_transition,
        atmosphere_blend,
        resolved_vp,
    };
    let (has_draws, has_patterned_bg) =
        enqueue_background_layers(world, style, view_state, &params, sky_metadata.is_some())?;
    if has_patterned_bg {
        if let Some(gpu) = world
            .resources
            .get_mut::<super::pattern::BackgroundPatternGpu>()
        {
            gpu.write(&renderer.queue, view_state, frame, uses_globe);
        }
    }
    if has_draws {
        upload_and_publish_background(
            world,
            style,
            view_state,
            renderer,
            &params,
            (sky_metadata, atmosphere_metadata),
        );
    }

    Ok(())
}

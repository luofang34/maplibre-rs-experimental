//! Surface fog, lighting, and procedural relief uniform assembly for terrain tiles.
use crate::{
    coords::WorldTileCoords,
    projection::globe::camera::GlobeCameraState,
    render::{settings::TerrainDrapeMode, view_state::ViewState},
    style::Style,
    terrain::resources::TerrainFog,
};

/// The fog of the frame from the style's sky and the view, as GL JS `terrainUniformValues`,
/// smoothly blended across the Mercator-to-globe transition and faded with altitude and
/// `atmosphere-blend` on the globe.
#[cfg(test)]
pub(crate) fn terrain_fog(style: &Style, view_state: &ViewState) -> TerrainFog {
    let pre = super::frame_globe_camera(style, view_state);
    terrain_fog_with_camera(style, view_state, pre.as_ref())
}

pub(super) fn terrain_fog_with_camera(
    style: &Style,
    view_state: &ViewState,
    precomputed: Option<&GlobeCameraState>,
) -> TerrainFog {
    use cgmath::InnerSpace;

    let Some(sky) = &style.sky else {
        return TerrainFog::default();
    };
    let zoom = view_state.zoom().value();
    let colors = sky.colors_at(view_state.style_zoom().value());
    let (near, far) = view_state.fog_depth_range();
    let meters = view_state.eye_fog_meters_per_pixel().unwrap_or(1.0);
    let (near, mut far) = (near * meters, far * meters);
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification.projection_type.globe_transition(zoom)
    });
    let external_or_free = view_state.has_external_view()
        || view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe;
    let mut opacity = view_state.fog_opacity();
    let globe = if external_or_free {
        transition > 0.0
    } else if transition > 0.0 {
        let globe_fade = precomputed.map_or(0.0, |camera| {
            let distance_radii = camera.camera_position().magnitude();
            let radius_px = camera.globe_radius_pixels();
            let horizon_dist_px =
                (distance_radii * distance_radii - 1.0).max(0.0).sqrt() * radius_px;
            if horizon_dist_px > near {
                let t = f64::from(transition);
                far = far.min(far * (1.0 - t) + horizon_dist_px * t);
            }
            let atm_h = 100_000.0 / camera.body().radius_meters;
            let alt = distance_radii - 1.0;
            let u = ((alt - atm_h) / (9.0 * atm_h)).clamp(0.0, 1.0) as f32;
            let inside_atmosphere = 1.0 - u * u * (3.0 - 2.0 * u);
            inside_atmosphere * (1.0 - sky.atmosphere_blend_at_zoom(zoom))
        });
        opacity *= 1.0 - transition + transition * globe_fade;
        opacity <= 0.0
    } else {
        false
    };
    TerrainFog {
        fog_color: colors.fog,
        horizon_color: colors.horizon,
        near: near as f32,
        far: far as f32,
        ground_blend: colors.fog_ground_blend,
        horizon_blend: colors.horizon_fog_blend,
        opacity,
        globe,
    }
}

/// Static per-tile procedural relief/grid uniform arrays.
pub(in crate::terrain::queue_system) fn procedural_uniforms(
    coords: WorldTileCoords,
    drape_mode: TerrainDrapeMode,
    radius_meters: f64,
) -> ([f32; 4], [f32; 4]) {
    match drape_mode {
        TerrainDrapeMode::ProceduralRelief(relief) => {
            let tiles_across = 2_f64.powi(i32::from(u8::from(coords.z)));
            let merc_u =
                std::f64::consts::PI * (1.0 - 2.0 * ((f64::from(coords.y) + 0.5) / tiles_across));
            let circumference = std::f64::consts::TAU * radius_meters;
            let tile_span_m = (circumference / tiles_across / merc_u.cosh()).max(1.0) as f32;
            (
                [
                    1.0,
                    f32::from(relief.hypsometric),
                    relief.slope_shading,
                    relief.reference_altitude_meters.unwrap_or(0.0),
                ],
                [
                    relief.grid_spacing_meters,
                    relief.grid_opacity,
                    f32::from(relief.reference_altitude_meters.is_some()),
                    tile_span_m,
                ],
            )
        }
        TerrainDrapeMode::Draped { .. } => ([0.0; 4], [0.0; 4]),
    }
}

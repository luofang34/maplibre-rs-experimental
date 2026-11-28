//! Terrain texture coordinates and uniforms shared across a stereo frame.
use cgmath::{InnerSpace, Matrix4, Vector3};

pub(crate) use super::steady_cache::PreparedTerrain;
use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::globe::camera::GlobeCameraState,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        resolved_view::ResolvedFrameView,
        settings::TerrainDrapeMode,
        view_state::ViewState,
    },
    schedule::system::SystemResult,
    style::Style,
    tcs::world::World,
    terrain::{
        resources::{TerrainFog, TerrainResources, TerrainTileUniforms},
        DrapePhase,
    },
};

#[derive(Clone, Default)]
pub(crate) struct TerrainEyeFrame(
    pub Vec<(WorldTileCoords, TerrainTileUniforms)>,
    pub(crate) Vec<Option<Matrix4<f64>>>,
);

/// Maps tile coordinates in `0..EXTENT` of `coords` to unit coordinates inside its DEM tile.
pub(crate) fn dem_matrix(coords: WorldTileCoords, dem: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(dem.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (dem.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (dem.y << delta)) / scale;
    Matrix4::from_translation(Vector3::new(origin_x, origin_y, 0.0))
        * Matrix4::from_nonuniform_scale(1.0 / (EXTENT * scale), 1.0 / (EXTENT * scale), 1.0)
}

/// Whether any ancestor of `coords` currently holds a drape texture.
pub(crate) fn has_present_ancestor(coords: WorldTileCoords, terrain: &TerrainResources) -> bool {
    let mut current = coords;
    while let Some(parent) = current.get_parent() {
        if terrain.drape_texture(parent).is_some() {
            return true;
        }
        current = parent;
    }
    false
}

/// Nearest ancestor of `coords` that holds a drape texture and satisfies `accept`.
pub(crate) fn nearest_present_ancestor(
    coords: WorldTileCoords,
    terrain: &TerrainResources,
    mut accept: impl FnMut(WorldTileCoords) -> bool,
) -> Option<WorldTileCoords> {
    let mut current = coords;
    while let Some(parent) = current.get_parent() {
        if terrain.drape_texture(parent).is_some() && accept(parent) {
            return Some(parent);
        }
        current = parent;
    }
    None
}

/// Maps unit coordinates of `coords` into the unit coordinates of its ancestor `source`.
pub(crate) fn drape_matrix(coords: WorldTileCoords, source: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(source.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (source.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (source.y << delta)) / scale;
    Matrix4::from_translation(Vector3::new(origin_x, origin_y, 0.0))
        * Matrix4::from_nonuniform_scale(1.0 / scale, 1.0 / scale, 1.0)
}

fn frame_globe_camera(style: &Style, view: &ViewState) -> Option<GlobeCameraState> {
    let uses_globe = style
        .projection
        .as_ref()
        .is_some_and(|spec| spec.projection_type.globe_transition(view.zoom().value()) > 0.0);
    uses_globe
        .then(|| crate::render::projection::globe_camera_for_view(view).ok())
        .flatten()
}

pub(crate) fn frame_view_projection_globe_and_eye(
    world: &World,
    style: &Style,
    view: &ViewState,
) -> (
    crate::render::camera::ViewProjection,
    Option<GlobeCameraState>,
    Option<Vector3<f64>>,
) {
    if let Some(resolved) = world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view))
    {
        let globe = resolved.globe_camera().cloned();
        return (
            resolved.gpu_view_projection,
            globe,
            Some(resolved.eye_position),
        );
    }
    (
        view.gpu_view_projection(),
        frame_globe_camera(style, view),
        None,
    )
}

/// Adds a draw of each surface tile for every copy of the world that shows it, shifted over by
/// the copy's whole worlds. Returns the draws' sources and coordinates placed.
pub(crate) fn with_world_copies(
    prepared: &mut PreparedTerrain,
    copies: Option<&rustc_hash::FxHashMap<WorldTileCoords, Vec<i32>>>,
    view: &ViewState,
) {
    let Some(copies) = copies.filter(|copies| !copies.is_empty()) else {
        return;
    };
    let projection = view.gpu_view_projection();
    let zoom = view.zoom();
    let base_len = prepared.sources.len();
    for index in 0..base_len {
        let (Some(entry), Some(base_block)) = (
            prepared.sources.get(index).copied(),
            prepared.uniforms.get(index).copied(),
        ) else {
            continue;
        };
        let dem_id = prepared.dem_ids.get(index).copied().flatten();
        let basis = prepared.globe_bases.get(index).copied().flatten();
        let coords = entry.1;
        let Some(wraps) = copies.get(&coords) else {
            continue;
        };
        let bounds = crate::coords::ZOOM_BOUNDS
            .get(usize::from(u8::from(coords.z)))
            .copied()
            .unwrap_or(1) as i32;
        for wrap in wraps {
            let shifted = WorldTileCoords {
                x: coords.x + wrap * bounds,
                ..coords
            };
            let mut block = base_block;
            block.transform = projection
                .to_model_view_projection(shifted.transform_for_zoom(zoom))
                .downcast()
                .into();
            block.tile_mercator_coords[0] += *wrap as f32;
            prepared.uniforms.push(block);
            prepared.sources.push(entry);
            prepared.dem_ids.push(dem_id);
            prepared.globe_bases.push(basis);
        }
    }
}

/// The fog of the frame from the style's sky and the view, as GL JS `terrainUniformValues`,
/// smoothly blended across the Mercator-to-globe transition and faded with altitude and
/// `atmosphere-blend` on the globe.
#[cfg(test)]
pub(crate) fn terrain_fog(style: &Style, view_state: &ViewState) -> TerrainFog {
    let pre = frame_globe_camera(style, view_state);
    terrain_fog_with_camera(style, view_state, pre.as_ref())
}

pub(crate) fn terrain_fog_with_camera(
    style: &Style,
    view_state: &ViewState,
    precomputed: Option<&GlobeCameraState>,
) -> TerrainFog {
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
pub(crate) fn procedural_uniforms(
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

fn set_globe_uniforms(
    block: &mut TerrainTileUniforms,
    tile: WorldTileCoords,
    view: &ViewState,
    globe_camera: Option<&GlobeCameraState>,
) -> Option<Matrix4<f64>> {
    let radius = view.body().radius_meters;
    let Some((basis, origin)) = compute_basis(tile, radius) else {
        block.globe_origin = [0.0; 4];
        return None;
    };
    apply_basis_or_clear(block, globe_camera, Some(&basis), origin)
}

/// Computes tile-local tangent basis and globe origin uniform for `tile` (`z >= 8`).
pub(crate) fn compute_basis(
    tile: WorldTileCoords,
    radius: f64,
) -> Option<(Matrix4<f64>, [f32; 4])> {
    // Coarse tiles and polar fans use the global sphere path. Their local angular span is large.
    if u8::from(tile.z) < 8 {
        return None;
    }
    let n = 2_f64.powi(i32::from(u8::from(tile.z)));
    let longitude = ((f64::from(tile.x) + 0.5) / n * 2.0 - 1.0) * std::f64::consts::PI;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(tile.y) + 0.5) / n))
        .sinh()
        .atan();
    let (s, c) = latitude.sin_cos();
    let (sl, cl) = longitude.sin_cos();
    let east = Vector3::new(cl, 0.0, -sl);
    let north = Vector3::new(-sl * s, c, -cl * s);
    let up = Vector3::new(sl * c, s, cl * c);
    let basis = Matrix4::from_cols(
        (east / radius).extend(0.0),
        (north / radius).extend(0.0),
        (up / radius).extend(0.0),
        up.extend(1.0),
    );
    let origin = [
        s as f32,
        c as f32,
        (std::f64::consts::TAU / (n * EXTENT)) as f32,
        radius as f32,
    ];
    Some((basis, origin))
}

/// Applies precomputed globe tangent basis into `block`, clearing `globe_origin` on fallback.
pub(crate) fn apply_basis_or_clear(
    block: &mut TerrainTileUniforms,
    camera: Option<&GlobeCameraState>,
    basis: Option<&Matrix4<f64>>,
    origin: [f32; 4],
) -> Option<Matrix4<f64>> {
    if let (Some(camera), Some(basis)) = (camera, basis)
        && let Some(matrix) = (camera.wgpu_view_projection() * basis).cast::<f32>()
    {
        block.globe_transform = matrix.into();
        block.globe_origin = origin;
        return Some(*basis);
    }
    block.globe_origin = [0.0; 4];
    None
}

/// Re-projects a tile's precomputed globe tangent basis for a stereo sibling eye without
/// re-evaluating `sinh`/`atan`/`sin_cos` trigonometry.
fn apply_globe_basis(
    block: &mut TerrainTileUniforms,
    camera: Option<&GlobeCameraState>,
    basis: Option<&Matrix4<f64>>,
) {
    let (Some(camera), Some(basis)) = (camera, basis) else {
        return;
    };
    if let Some(matrix) = (camera.wgpu_view_projection() * basis).cast::<f32>() {
        block.globe_transform = matrix.into();
    }
}

pub(crate) fn replay(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
    hide_draped_layers: impl FnOnce(&mut World, &Style),
) -> SystemResult {
    let (projection, globe_camera, resolved_eye) =
        frame_view_projection_globe_and_eye(world, style, view);
    let fog = terrain_fog_with_camera(style, view, globe_camera.as_ref());
    let eye_fog = view.eye_fog_params(resolved_eye);
    let lighting = world
        .resources
        .get::<crate::render::SceneLighting>()
        .copied()
        .unwrap_or_default();
    let zoom = view.zoom();
    let external = view.has_external_view();
    let relief_strength = if external { 0.45 } else { 0.0 };
    let external_flag = f32::from(external);
    let daylight = lighting.daylight_uniform();
    if let Some(frame) = world.resources.get_mut::<TerrainEyeFrame>() {
        for (idx, (coords, uniform)) in frame.0.iter_mut().enumerate() {
            uniform.transform = projection
                .to_model_view_projection(coords.transform_for_zoom(zoom))
                .downcast()
                .into();
            uniform.fog_color = fog.fog_color;
            uniform.horizon_color = fog.horizon_color;
            uniform.fog_range = [fog.near, fog.far, fog.ground_blend, fog.horizon_blend];
            uniform.fog_opacity[0] = fog.opacity;
            uniform.fog_opacity[1] = f32::from(fog.globe);
            uniform.fog_opacity[3] = external_flag;
            uniform.relief_strength = relief_strength;
            uniform.daylight = daylight;
            uniform.twilight = lighting.twilight;
            uniform.moonlight = lighting.moonlight;
            uniform.fog_position = eye_fog.map_or([0.0; 4], |params| params.for_tile(*coords));
            let basis = frame.1.get(idx).and_then(Option::as_ref);
            if basis.is_some() {
                apply_globe_basis(uniform, globe_camera.as_ref(), basis);
            } else {
                set_globe_uniforms(uniform, *coords, view, globe_camera.as_ref());
            }
        }
    }
    if let Some((Initialized(terrain), frame)) = world
        .resources
        .query_mut::<(&mut Eventually<TerrainResources>, &TerrainEyeFrame)>()
    {
        terrain.replay_uniforms(queue, &frame.0);
    }
    world.resources.insert(DrapePhase::default());
    hide_draped_layers(world, style);
    Ok(())
}

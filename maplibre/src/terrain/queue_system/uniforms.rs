//! Terrain texture coordinates and uniforms shared across a stereo frame.
use cgmath::{Matrix4, SquareMatrix};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        resolved_view::{ProjectionMode, ResolvedFrameView},
        view_state::ViewState,
    },
    style::{source::TileAddressingScheme, Style},
    tcs::world::World,
    terrain::{
        resources::{TerrainFog, TerrainResources, TerrainTileUniforms},
        source::DemSource,
    },
};
mod fog;
mod globe;
mod polar;
mod replay;
#[cfg(test)]
pub(crate) use fog::terrain_fog;
use fog::terrain_fog_with_camera;
pub(super) use replay::replay;
pub(crate) use replay::TerrainEyeFrame;

/// Maps tile coordinates in `0..EXTENT` of `coords` to unit coordinates inside its DEM tile.
fn dem_matrix(coords: WorldTileCoords, dem: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(dem.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (dem.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (dem.y << delta)) / scale;
    Matrix4::from_translation(cgmath::Vector3::new(origin_x, origin_y, 0.0))
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
fn drape_matrix(coords: WorldTileCoords, source: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(source.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (source.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (source.y << delta)) / scale;
    Matrix4::from_translation(cgmath::Vector3::new(origin_x, origin_y, 0.0))
        * Matrix4::from_nonuniform_scale(1.0 / scale, 1.0 / scale, 1.0)
}

type TileSources = (
    Option<WorldTileCoords>,
    WorldTileCoords,
    Option<WorldTileCoords>,
);

#[derive(Default)]
pub(super) struct PreparedTerrain {
    pub(super) uniforms: Vec<TerrainTileUniforms>,
    pub(super) sources: Vec<TileSources>,
    pub(super) dem_ids: Vec<Option<u64>>,
    pub(super) globe_bases: Vec<Option<Matrix4<f64>>>,
}

impl PreparedTerrain {
    pub(super) fn clear(&mut self) {
        self.uniforms.clear();
        self.sources.clear();
        self.dem_ids.clear();
        self.globe_bases.clear();
    }

    fn reserve(&mut self, additional: usize) {
        self.uniforms.reserve(additional);
        self.sources.reserve(additional);
        self.dem_ids.reserve(additional);
        self.globe_bases.reserve(additional);
    }
}

fn frame_globe_camera(
    style: &Style,
    view: &ViewState,
) -> Option<crate::projection::globe::camera::GlobeCameraState> {
    let uses_globe = style
        .projection
        .as_ref()
        .is_some_and(|spec| spec.projection_type.globe_transition(view.zoom().value()) > 0.0);
    uses_globe
        .then(|| crate::render::projection::globe_camera_for_view(view).ok())
        .flatten()
}

pub(super) fn frame_view_projection_globe_and_eye(
    world: &World,
    style: &Style,
    view: &ViewState,
) -> (
    crate::render::camera::ViewProjection,
    Option<crate::projection::globe::camera::GlobeCameraState>,
    Option<cgmath::Vector3<f64>>,
) {
    if let Some(resolved) = world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view))
    {
        let globe = match &resolved.projection {
            ProjectionMode::Globe { camera, .. } => Some((**camera).clone()),
            ProjectionMode::FlatMercator => None,
        };
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

pub(super) struct SurfaceContext<'a> {
    style: &'a Style,
    view_state: &'a ViewState,
    dem: &'a DemSource,
    skirts: crate::terrain::TerrainSkirts,
    poles: crate::terrain::PolarPoleElevations,
    lighting: crate::render::SceneLighting,
    paint_zoom: f64,
    clear_color: [f32; 4],
    vector_arrival_epoch: u64,
    has_external_view: bool,
    eye_fog: Option<crate::render::view_state::EyeFogParams>,
    gpu_view_projection: crate::render::camera::ViewProjection,
    globe_camera: Option<crate::projection::globe::camera::GlobeCameraState>,
}

/// Captures style, view, DEM and host lighting inputs for preparing terrain tile uniforms.
pub(super) fn surface_context<'a>(
    world: &World,
    style: &'a Style,
    view_state: &'a ViewState,
    dem: &'a DemSource,
    clear: wgpu::Color,
) -> SurfaceContext<'a> {
    let skirts = world.resources.get::<crate::terrain::TerrainSkirts>();
    let poles = world
        .resources
        .get::<crate::terrain::PolarPoleElevations>()
        .copied()
        .unwrap_or_default();
    let lighting = world
        .resources
        .get::<crate::render::SceneLighting>()
        .copied()
        .unwrap_or_default();
    let paint_zoom =
        crate::vector::upload_system::paint::current_zoom(world, view_state.style_zoom().value());
    let clear_color = [
        clear.r as f32,
        clear.g as f32,
        clear.b as f32,
        clear.a as f32,
    ];
    let (gpu_view_projection, globe_camera, resolved_eye) =
        frame_view_projection_globe_and_eye(world, style, view_state);
    let eye_fog = view_state.eye_fog_params(resolved_eye);
    SurfaceContext {
        style,
        view_state,
        dem,
        skirts: skirts.copied().unwrap_or_default(),
        poles,
        lighting,
        paint_zoom,
        clear_color,
        vector_arrival_epoch: crate::vector::arrival_epoch(world),
        has_external_view: view_state.has_external_view(),
        eye_fog,
        gpu_view_projection,
        globe_camera,
    }
}

pub(super) fn prepare_tiles_into(
    out: &mut PreparedTerrain,
    static_cache: &mut super::steady_cache::TileStaticUniformCache,
    targets: (&[WorldTileCoords], &[Option<WorldTileCoords>]),
    terrain: &TerrainResources,
    tiles: &crate::tcs::tiles::Tiles,
    ctx: SurfaceContext<'_>,
) {
    let (coords_list, drape_sources) = targets;
    let zoom = ctx.view_state.zoom();
    let fog = terrain_fog_with_camera(ctx.style, ctx.view_state, ctx.globe_camera.as_ref());
    let skirt_length = match ctx.skirts {
        crate::terrain::TerrainSkirts::Auto => {
            ctx.view_state.body().circumference_meters()
                / 2_f64.powf(zoom.value().max(0.0))
                / super::SKIRT_DIVISOR
        }
        // A skirt of no length folds onto the tile edge and covers no pixel.
        crate::terrain::TerrainSkirts::None => 0.0,
    };
    let any_polar = coords_list.iter().any(|coords| {
        let max_row = (1_i32
            .checked_shl(u32::from(u8::from(coords.z)))
            .unwrap_or(1))
            - 1;
        coords.y == 0 || coords.y == max_row
    });
    let vector_tiles_len = if any_polar {
        tiles
            .pyramid_of::<crate::vector::VectorLayerBucketComponent>()
            .len()
    } else {
        0
    };
    static_cache.begin_frame(super::steady_cache::StaticFrameKey {
        style_rev: ctx.style.revision(),
        paint_zoom_bits: if any_polar {
            ctx.paint_zoom.to_bits()
        } else {
            0
        },
        clear_color_bits: ctx.clear_color.map(f32::to_bits),
        poles_north_bits: ctx.poles.north_meters.to_bits(),
        poles_south_bits: ctx.poles.south_meters.to_bits(),
        drape_mode_bits: super::steady_cache::StaticFrameKey::drape_mode_bits(terrain.drape_mode()),
        dem_exaggeration_bits: ctx.dem.exaggeration.to_bits(),
        dem_unpack_bits: ctx.dem.unpack.map(f64::to_bits),
        radius_bits: ctx.view_state.body().radius_meters.to_bits(),
        has_globe_camera: ctx.globe_camera.is_some(),
        vector_arrival_epoch: if any_polar {
            ctx.vector_arrival_epoch
        } else {
            0
        },
        vector_tiles_len,
    });
    out.clear();
    out.reserve(coords_list.len());
    for (&coords, &drape_source) in coords_list.iter().zip(drape_sources) {
        if let Some((block, tile_source, dem_id, basis)) = prepare_single_tile(
            coords,
            drape_source,
            terrain,
            tiles,
            &ctx,
            static_cache,
            (skirt_length as f32, &fog),
        ) {
            out.uniforms.push(block);
            out.sources.push(tile_source);
            out.dem_ids.push(dem_id);
            out.globe_bases.push(basis);
        }
    }
}

type PreparedSingleTile = (
    TerrainTileUniforms,
    TileSources,
    Option<u64>,
    Option<Matrix4<f64>>,
);

fn build_tile_static(
    coords: WorldTileCoords,
    (dem_coords, dem_dim, drape_source): (Option<WorldTileCoords>, f32, Option<WorldTileCoords>),
    drape_mode: crate::render::settings::TerrainDrapeMode,
    tiles: &crate::tcs::tiles::Tiles,
    ctx: &SurfaceContext<'_>,
) -> Option<super::steady_cache::CachedTileStatic> {
    let (dem_mat, dem_unpack) = match dem_coords {
        Some(dc) => (
            dem_matrix(coords, dc).cast::<f32>()?,
            ctx.dem.unpack.map(|value| value as f32),
        ),
        None => (Matrix4::identity(), [0.0; 4]),
    };
    let drape_mat = drape_source
        .map_or_else(Matrix4::identity, |source| drape_matrix(coords, source))
        .cast::<f32>()?;
    let [north_cap, south_cap] = polar::cap_colors_with_base(
        coords,
        drape_source,
        ctx.style,
        tiles,
        ctx.paint_zoom,
        ctx.clear_color,
    );
    let max_row = 1_i32
        .checked_shl(u32::from(u8::from(coords.z)))
        .unwrap_or(1)
        - 1;
    let pole_elev = if coords.y == 0 {
        ctx.poles.north_meters * ctx.dem.exaggeration
    } else if coords.y == max_row {
        ctx.poles.south_meters * ctx.dem.exaggeration
    } else {
        0.0
    };
    let (globe_basis, globe_origin) = if ctx.globe_camera.is_some() {
        globe::compute_basis(coords, ctx.view_state.body().radius_meters)
            .map_or((None, [0.0; 4]), |(basis, origin)| (Some(basis), origin))
    } else {
        (None, [0.0; 4])
    };
    let (procedural_relief, procedural_grid) =
        fog::procedural_uniforms(coords, drape_mode, ctx.view_state.body().radius_meters);
    Some(super::steady_cache::CachedTileStatic {
        dem_matrix: dem_mat.into(),
        drape_matrix: drape_mat.into(),
        tile_mercator_coords: tile_mercator_coordinates(
            coords.into_tile(TileAddressingScheme::XYZ),
        )
        .into(),
        dem_unpack,
        dem_dim,
        exaggeration: ctx.dem.exaggeration,
        edge_index: [0, north_cap, south_cap, pole_elev.to_bits()],
        surface_color: ctx.clear_color,
        globe_origin,
        globe_basis,
        procedural_relief,
        procedural_grid,
        last_used_frame: 0,
    })
}

fn prepare_single_tile(
    coords: WorldTileCoords,
    drape_source: Option<WorldTileCoords>,
    terrain: &TerrainResources,
    tiles: &crate::tcs::tiles::Tiles,
    ctx: &SurfaceContext<'_>,
    static_cache: &mut super::steady_cache::TileStaticUniformCache,
    (skirt_length, fog): (f32, &TerrainFog),
) -> Option<PreparedSingleTile> {
    let zoom = ctx.view_state.zoom();
    let (dem_coords, dem_id, dem_dim) = match terrain.resolve_loaded_dem(coords, ctx.dem) {
        Some((dem_coords, id, dim)) => (Some(dem_coords), Some(id), dim),
        None => (None, None, 1.0),
    };
    let key = (coords, dem_coords, dem_id, drape_source);
    let cached = static_cache.get_or_try_insert_with(key, || {
        build_tile_static(
            coords,
            (dem_coords, dem_dim, drape_source),
            terrain.drape_mode(),
            tiles,
            ctx,
        )
    })?;
    let mut block = TerrainTileUniforms {
        transform: ctx
            .gpu_view_projection
            .to_model_view_projection(coords.transform_for_zoom(zoom))
            .downcast()
            .into(),
        globe_transform: Matrix4::identity().into(),
        globe_origin: [0.0; 4],
        dem_matrix: cached.dem_matrix,
        drape_matrix: cached.drape_matrix,
        tile_mercator_coords: cached.tile_mercator_coords,
        dem_unpack: cached.dem_unpack,
        dem_dim: cached.dem_dim,
        exaggeration: cached.exaggeration,
        skirt_length,
        relief_strength: if ctx.has_external_view { 0.45 } else { 0.0 },
        fog_color: fog.fog_color,
        horizon_color: fog.horizon_color,
        fog_range: [fog.near, fog.far, fog.ground_blend, fog.horizon_blend],
        fog_opacity: [
            fog.opacity,
            f32::from(fog.globe),
            f32::from(drape_source.is_none()),
            f32::from(ctx.has_external_view),
        ],
        edge_index: cached.edge_index,
        surface_color: cached.surface_color,
        fog_position: ctx
            .eye_fog
            .map_or([0.0; 4], |params| params.for_tile(coords)),
        daylight: ctx.lighting.daylight_uniform(),
        twilight: ctx.lighting.twilight,
        moonlight: ctx.lighting.moonlight,
        procedural_relief: cached.procedural_relief,
        procedural_grid: cached.procedural_grid,
    };
    let basis = globe::apply_basis_or_clear(
        &mut block,
        ctx.globe_camera.as_ref(),
        cached.globe_basis.as_ref(),
        cached.globe_origin,
    );
    Some((block, (dem_coords, coords, drape_source), dem_id, basis))
}

/// Adds a draw of each surface tile for every copy of the world that shows it, shifted over by
/// the copy's whole worlds. Returns the draws' sources and coordinates placed.
pub(super) fn with_world_copies(
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

//! Style/vector/polar uniform preparation for terrain surface tiles.

use cgmath::{Matrix4, SquareMatrix};

use super::polar;
use crate::{
    coords::WorldTileCoords,
    projection::renderer_data::tile_mercator_coordinates,
    render::view_state::ViewState,
    style::{source::TileAddressingScheme, Style},
    tcs::world::World,
    terrain::{
        queue_system::{
            steady_cache::{
                CachedTileStatic, PreparedTerrain, StaticFrameKey, TileStaticUniformCache,
            },
            uniforms::{
                apply_basis_or_clear, compute_basis, dem_matrix, drape_matrix,
                frame_view_projection_globe_and_eye, procedural_uniforms, terrain_fog_with_camera,
            },
            SKIRT_DIVISOR,
        },
        resources::{TerrainFog, TerrainResources, TerrainTileUniforms},
        source::DemSource,
    },
};

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
    let paint_zoom = crate::tcs::paint_zoom::current_zoom(world, view_state.style_zoom().value());
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
        vector_arrival_epoch: crate::tcs::tile_epochs::vector_arrival_epoch(world),
        has_external_view: view_state.has_external_view(),
        eye_fog,
        gpu_view_projection,
        globe_camera,
    }
}

type TileSources = (
    Option<WorldTileCoords>,
    WorldTileCoords,
    Option<WorldTileCoords>,
);

type PreparedSingleTile = (
    TerrainTileUniforms,
    TileSources,
    Option<u64>,
    Option<Matrix4<f64>>,
);

pub(super) fn prepare_tiles_into(
    out: &mut PreparedTerrain,
    static_cache: &mut TileStaticUniformCache,
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
                / SKIRT_DIVISOR
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
    static_cache.begin_frame(StaticFrameKey {
        style_rev: ctx.style.revision(),
        paint_zoom_bits: if any_polar {
            ctx.paint_zoom.to_bits()
        } else {
            0
        },
        clear_color_bits: ctx.clear_color.map(f32::to_bits),
        poles_north_bits: ctx.poles.north_meters.to_bits(),
        poles_south_bits: ctx.poles.south_meters.to_bits(),
        drape_mode_bits: StaticFrameKey::drape_mode_bits(terrain.drape_mode()),
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

fn build_tile_static(
    coords: WorldTileCoords,
    (dem_coords, dem_dim, drape_source): (Option<WorldTileCoords>, f32, Option<WorldTileCoords>),
    drape_mode: crate::render::settings::TerrainDrapeMode,
    tiles: &crate::tcs::tiles::Tiles,
    ctx: &SurfaceContext<'_>,
) -> Option<CachedTileStatic> {
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
        compute_basis(coords, ctx.view_state.body().radius_meters)
            .map_or((None, [0.0; 4]), |(basis, origin)| (Some(basis), origin))
    } else {
        (None, [0.0; 4])
    };
    let (procedural_relief, procedural_grid) =
        procedural_uniforms(coords, drape_mode, ctx.view_state.body().radius_meters);
    Some(CachedTileStatic {
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
    static_cache: &mut TileStaticUniformCache,
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
    let basis = apply_basis_or_clear(
        &mut block,
        ctx.globe_camera.as_ref(),
        cached.globe_basis.as_ref(),
        cached.globe_origin,
    );
    Some((block, (dem_coords, coords, drape_source), dem_id, basis))
}

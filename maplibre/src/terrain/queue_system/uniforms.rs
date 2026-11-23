//! Terrain texture coordinates and uniforms shared across a stereo frame.
use cgmath::{Matrix4, SquareMatrix, Vector3};

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
use fog::{surface_uniforms, terrain_fog_with_camera};
pub(super) use replay::replay;
pub(crate) use replay::TerrainEyeFrame;

/// Maps tile coordinates in `0..EXTENT` of `coords` to unit coordinates inside its DEM tile.
fn dem_matrix(coords: WorldTileCoords, dem: WorldTileCoords) -> Matrix4<f64> {
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
fn drape_matrix(coords: WorldTileCoords, source: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(source.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (source.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (source.y << delta)) / scale;
    Matrix4::from_translation(Vector3::new(origin_x, origin_y, 0.0))
        * Matrix4::from_nonuniform_scale(1.0 / scale, 1.0 / scale, 1.0)
}

/// The tiles whose textures a terrain tile samples.
pub(super) struct TileTextures {
    /// The DEM tile, or none while no DEM covers the tile.
    pub(super) dem: Option<WorldTileCoords>,
    /// Interior DEM sample side length in pixels.
    pub(super) dem_dim: f32,
    /// The tile whose drape is shown, an ancestor's until the tile's own is drawn.
    pub(super) drape: Option<WorldTileCoords>,
}

pub(super) fn tile_uniforms(
    coords: WorldTileCoords,
    textures: TileTextures,
    _terrain: &TerrainResources,
    dem: &DemSource,
    transform: &Matrix4<f32>,
    skirt_length: f32,
    fog: &TerrainFog,
) -> Option<TerrainTileUniforms> {
    let (dem_matrix, dem_unpack, dem_dim) = match textures.dem {
        Some(dem_coords) => (
            dem_matrix(coords, dem_coords).cast::<f32>()?,
            dem.unpack.map(|value| value as f32),
            textures.dem_dim,
        ),
        None => (Matrix4::identity(), [0.0; 4], 1.0),
    };
    Some(TerrainTileUniforms {
        transform: (*transform).into(),
        globe_transform: Matrix4::identity().into(),
        globe_origin: [0.0; 4],
        dem_matrix: dem_matrix.into(),
        drape_matrix: textures
            .drape
            .map_or_else(Matrix4::identity, |source| drape_matrix(coords, source))
            .cast::<f32>()?
            .into(),
        tile_mercator_coords: tile_mercator_coordinates(
            coords.into_tile(TileAddressingScheme::XYZ),
        )
        .into(),
        dem_unpack,
        dem_dim,
        exaggeration: dem.exaggeration,
        skirt_length,
        relief_strength: 0.0,
        fog_color: fog.fog_color,
        horizon_color: fog.horizon_color,
        fog_range: [fog.near, fog.far, fog.ground_blend, fog.horizon_blend],
        fog_opacity: [
            fog.opacity,
            f32::from(fog.globe),
            f32::from(textures.drape.is_none()),
            0.0,
        ],
        edge_index: [0; 4],
        surface_color: [0.0; 4],
        fog_position: [0.0; 4],
        daylight: [0.0; 4],
        twilight: [0.0; 4],
        moonlight: [0.0; 4],
        procedural_relief: [0.0; 4],
        procedural_grid: [0.0; 4],
    })
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

pub(super) fn frame_view_projection_and_globe(
    world: &World,
    style: &Style,
    view: &ViewState,
) -> (
    crate::render::camera::ViewProjection,
    Option<crate::projection::globe::camera::GlobeCameraState>,
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
        return (resolved.gpu_view_projection, globe);
    }
    (view.gpu_view_projection(), frame_globe_camera(style, view))
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
    gpu_view_projection: crate::render::camera::ViewProjection,
    globe_camera: Option<crate::projection::globe::camera::GlobeCameraState>,
}

/// Captures style, view, DEM and host lighting inputs for preparing terrain tile uniforms.
pub(super) fn surface_context<'a>(
    world: &World,
    style: &'a Style,
    view_state: &'a ViewState,
    dem: &'a DemSource,
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
    let clear = super::drape_phase::background_clear_color(style);
    let clear_color = [
        clear.r as f32,
        clear.g as f32,
        clear.b as f32,
        clear.a as f32,
    ];
    let (gpu_view_projection, globe_camera) =
        frame_view_projection_and_globe(world, style, view_state);
    SurfaceContext {
        style,
        view_state,
        dem,
        skirts: skirts.copied().unwrap_or_default(),
        poles,
        lighting,
        paint_zoom,
        clear_color,
        gpu_view_projection,
        globe_camera,
    }
}

pub(super) fn prepare_tiles_into(
    out: &mut PreparedTerrain,
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
    out.clear();
    out.reserve(coords_list.len());
    for (&coords, &drape_source) in coords_list.iter().zip(drape_sources) {
        if let Some((block, tile_source, dem_id, basis)) = prepare_single_tile(
            coords,
            drape_source,
            terrain,
            tiles,
            &ctx,
            (&ctx.gpu_view_projection, skirt_length as f32, &fog),
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

fn prepare_single_tile(
    coords: WorldTileCoords,
    drape_source: Option<WorldTileCoords>,
    terrain: &TerrainResources,
    tiles: &crate::tcs::tiles::Tiles,
    ctx: &SurfaceContext<'_>,
    (gpu_view_projection, skirt_length, fog): (
        &crate::render::camera::ViewProjection,
        f32,
        &TerrainFog,
    ),
) -> Option<PreparedSingleTile> {
    let zoom = ctx.view_state.zoom();
    let (dem_coords, dem_id, dem_dim) = match terrain.resolve_loaded_dem(coords, ctx.dem) {
        Some((dem_coords, id, dim)) => (Some(dem_coords), Some(id), dim),
        None => (None, None, 1.0),
    };
    let mut block = tile_uniforms(
        coords,
        TileTextures {
            dem: dem_coords,
            dem_dim,
            drape: drape_source,
        },
        terrain,
        ctx.dem,
        &gpu_view_projection
            .to_model_view_projection(coords.transform_for_zoom(zoom))
            .downcast(),
        skirt_length,
        fog,
    )?;
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
    block.edge_index = [0, north_cap, south_cap, pole_elev.to_bits()];
    let basis = surface_uniforms(
        &mut block,
        coords,
        (ctx.clear_color, ctx.lighting, terrain.drape_mode()),
        ctx.view_state,
        ctx.globe_camera.as_ref(),
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

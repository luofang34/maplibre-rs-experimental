//! Terrain texture coordinates and uniforms shared across a stereo frame.
use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        view_state::ViewState,
    },
    style::{source::TileAddressingScheme, Style},
    tcs::{system::SystemResult, world::World},
    terrain::{
        drape_targets::TargetSpec,
        request_system::dem_tile_coords,
        resources::{TerrainFog, TerrainResources, TerrainTileUniforms},
        source::DemSource,
        DrapePhase,
    },
};
use cgmath::{Matrix4, SquareMatrix, Vector3};
#[derive(Default)]
pub(super) struct TerrainEyeFrame(pub Vec<(WorldTileCoords, TerrainTileUniforms)>);

pub(super) fn replay(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    queue: &wgpu::Queue,
) -> SystemResult {
    let fog = terrain_fog(style, view);
    let projection = view.gpu_view_projection();
    if let Some(frame) = world.resources.get_mut::<TerrainEyeFrame>() {
        for (coords, uniform) in &mut frame.0 {
            uniform.transform = projection
                .to_model_view_projection(coords.transform_for_zoom(view.zoom()))
                .downcast()
                .into();
            uniform.fog_color = fog.fog_color;
            uniform.horizon_color = fog.horizon_color;
            uniform.fog_range = [fog.near, fog.far, fog.ground_blend, fog.horizon_blend];
            uniform.fog_opacity[0] = fog.opacity;
            uniform.fog_opacity[1] = f32::from(fog.globe);
            surface_uniforms(uniform, *coords, style, view);
        }
    }
    if let (Some(frame), Some(Initialized(terrain))) = (
        world.resources.get::<TerrainEyeFrame>(),
        world.resources.get::<Eventually<TerrainResources>>(),
    ) {
        let uniforms: Vec<_> = frame.0.iter().map(|(_, uniform)| *uniform).collect();
        terrain.write_uniforms(queue, &uniforms);
    }
    world.resources.insert(DrapePhase::default());
    super::hide_draped_layers(world, style);
    Ok(())
}
/// The nearest uploaded DEM tile at or above the DEM zoom of a view tile.
pub(super) fn loaded_dem_tile(
    coords: WorldTileCoords,
    dem: &DemSource,
    terrain: &TerrainResources,
) -> Option<WorldTileCoords> {
    let mut current = dem_tile_coords(coords, dem.minzoom, dem.maxzoom)?;
    loop {
        if terrain.has_dem_texture(current) {
            return Some(current);
        }
        current = current.get_parent()?;
    }
}

/// Maps tile coordinates in `0..EXTENT` of `coords` to unit coordinates inside its DEM tile.
fn dem_matrix(coords: WorldTileCoords, dem: WorldTileCoords) -> Matrix4<f64> {
    let delta = i32::from(u8::from(coords.z)) - i32::from(u8::from(dem.z));
    let scale = 2_f64.powi(delta);
    let origin_x = f64::from(coords.x - (dem.x << delta)) / scale;
    let origin_y = f64::from(coords.y - (dem.y << delta)) / scale;
    Matrix4::from_translation(Vector3::new(origin_x, origin_y, 0.0))
        * Matrix4::from_nonuniform_scale(1.0 / (EXTENT * scale), 1.0 / (EXTENT * scale), 1.0)
}

/// The ancestors of a tile, nearest first, that hold a drape texture.
pub(super) fn present_ancestors(
    coords: WorldTileCoords,
    terrain: &TerrainResources,
) -> Vec<WorldTileCoords> {
    let mut ancestors = Vec::new();
    let mut current = coords;
    while let Some(parent) = current.get_parent() {
        if terrain.drape_texture(parent).is_some() {
            ancestors.push(parent);
        }
        current = parent;
    }
    ancestors
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
    /// The tile whose drape is shown, an ancestor's until the tile's own is drawn.
    pub(super) drape: Option<WorldTileCoords>,
}

pub(super) fn tile_uniforms(
    coords: WorldTileCoords,
    textures: TileTextures,
    terrain: &TerrainResources,
    dem: &DemSource,
    transform: &Matrix4<f32>,
    skirt_length: f32,
    fog: &TerrainFog,
) -> Option<TerrainTileUniforms> {
    let (dem_matrix, dem_unpack, dem_dim) = match textures.dem {
        Some(dem_coords) => (
            dem_matrix(coords, dem_coords).cast::<f32>()?,
            dem.unpack.map(|value| value as f32),
            (terrain
                .dem_texture(Some(dem_coords))
                .size
                .width
                .saturating_sub(2))
            .max(1) as f32,
        ),
        None => (Matrix4::identity(), [0.0; 4], 1.0),
    };
    Some(TerrainTileUniforms {
        transform: (*transform).into(),
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
        surface_color: [0.0; 4],
        fog_position: [0.0; 4],
    })
}

/// The fog of the frame from the style's sky and the view, as GL JS `terrainUniformValues`.
pub(super) fn terrain_fog(style: &Style, view_state: &ViewState) -> TerrainFog {
    let Some(sky) = &style.sky else {
        return TerrainFog::default();
    };
    let colors = sky.colors_at(view_state.style_zoom().value());
    let (near, far) = view_state.fog_depth_range();
    let meters = view_state.eye_fog_meters_per_pixel().unwrap_or(1.0);
    let (near, far) = (near * meters, far * meters);
    let globe = style.projection.as_ref().is_some_and(|specification| {
        specification
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });
    TerrainFog {
        fog_color: colors.fog,
        horizon_color: colors.horizon,
        near: near as f32,
        far: far as f32,
        ground_blend: colors.fog_ground_blend,
        horizon_blend: colors.horizon_fog_blend,
        opacity: view_state.fog_opacity(),
        globe,
    }
}

type TileSources = (
    Option<WorldTileCoords>,
    WorldTileCoords,
    Option<WorldTileCoords>,
);

pub(super) struct PreparedTerrain {
    pub(super) uniforms: Vec<TerrainTileUniforms>,
    pub(super) sources: Vec<TileSources>,
}

pub(super) fn prepare_tiles(
    targets: (&[TargetSpec], &[Option<WorldTileCoords>]),
    style: &Style,
    view_state: &ViewState,
    terrain: &TerrainResources,
    dem: &DemSource,
) -> PreparedTerrain {
    let (specs, drape_sources) = targets;
    let zoom = view_state.zoom();
    let gpu_view_projection = view_state.gpu_view_projection();
    let fog = terrain_fog(style, view_state);
    let skirt_length = view_state.body().circumference_meters()
        / 2_f64.powf(zoom.value().max(0.0))
        / super::SKIRT_DIVISOR;
    let mut uniforms = Vec::with_capacity(specs.len());
    let mut sources = Vec::with_capacity(specs.len());
    for (spec, drape_source) in specs.iter().zip(drape_sources) {
        let drape_source = *drape_source;
        let dem_coords = loaded_dem_tile(spec.coords, dem, terrain);
        let Some(mut block) = tile_uniforms(
            spec.coords,
            TileTextures {
                dem: dem_coords,
                drape: drape_source,
            },
            terrain,
            dem,
            &gpu_view_projection
                .to_model_view_projection(spec.coords.transform_for_zoom(zoom))
                .downcast(),
            skirt_length as f32,
            &fog,
        ) else {
            continue;
        };
        surface_uniforms(&mut block, spec.coords, style, view_state);
        uniforms.push(block);
        sources.push((dem_coords, spec.coords, drape_source));
    }
    PreparedTerrain { uniforms, sources }
}

fn surface_uniforms(
    block: &mut TerrainTileUniforms,
    coords: WorldTileCoords,
    style: &Style,
    view: &ViewState,
) {
    let color = super::background_clear_color(style);
    block.surface_color = [
        color.r as f32,
        color.g as f32,
        color.b as f32,
        color.a as f32,
    ];
    block.relief_strength = if view.has_external_view() { 0.45 } else { 0.0 };
    block.fog_opacity[3] = f32::from(view.has_external_view());
    block.fog_position = view.eye_fog_position(coords).unwrap_or([0.0; 4]);
}

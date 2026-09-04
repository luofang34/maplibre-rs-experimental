//! Builds the drape targets and terrain draws for the current frame.

use std::collections::HashSet;

use cgmath::{Matrix4, SquareMatrix, Vector3};

use crate::{
    context::MapContext,
    coords::{WorldTileCoords, Zoom, EXTENT, TILE_SIZE},
    projection::{globe::EARTH_RADIUS_METERS, renderer_data::tile_mercator_coordinates},
    raster::{render_commands::DrawRasterTiles, resource::RasterResources},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::view_region_for_projection,
        render_commands::DrawMasks,
        render_phase::{Draw, DrawState, LayerItem, ProjectionBinding, RenderPhase, TileMaskItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{
            HasTile, TileShape, ViewTileSources, WgpuTileViewPattern, DEFAULT_TILE_SIZE,
        },
        view_state::ViewStatePadding,
        Renderer,
    },
    style::{source::TileAddressingScheme, Style},
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
        world::World,
    },
    terrain::{
        request_system::dem_tile_coords,
        resources::{
            TerrainDraw, TerrainResources, TerrainTileUniforms, DRAPE_SIZE, UNIFORM_STRIDE,
        },
        rtt::drape_transform,
        source::{dem_source, DemSource},
        DrapePhase, DrapeTarget, TerrainFrame,
    },
    vector::{
        render_commands::{DrawLineTiles, DrawVectorTiles},
        VectorBufferPool,
    },
};

/// Divisor of the tile circumference giving the skirt drop, as in GL JS `getSkirtLength`.
const SKIRT_DIVISOR: f64 = 5.0;
/// How many zoom levels below a target tile children are searched for source data.
const CHILDREN_SEARCH_DEPTH: usize = 4;
const DRAPEABLE_LAYER_TYPES: [&str; 3] = ["fill", "line", "raster"];

/// Whether a style layer renders into drape textures rather than straight to the screen.
pub fn is_drapeable(layer_type: &str) -> bool {
    DRAPEABLE_LAYER_TYPES.contains(&layer_type)
}

struct VectorLayerSpec {
    id: String,
    index: u32,
    is_line: bool,
    coords: WorldTileCoords,
}

struct ShapeSpec {
    source: WorldTileCoords,
    vector_layers: Vec<VectorLayerSpec>,
    raster_layers: Vec<(String, u32)>,
}

struct TargetSpec {
    coords: WorldTileCoords,
    shapes: Vec<ShapeSpec>,
}

pub fn queue_system(
    MapContext {
        style,
        view_state,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(dem) = dem_source(style) else {
        world.resources.insert(DrapePhase::default());
        world.resources.insert(TerrainFrame::default());
        return Ok(());
    };
    let zoom = view_state.zoom();
    let Some(view_region) = view_region_for_projection(
        style,
        view_state,
        world,
        zoom.zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Tight,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select terrain tiles");
        SystemError::Setup
    })?
    else {
        return Ok(());
    };

    let targets = select_targets(view_region.iter(), world);
    let specs = collect_layer_specs(targets, style, world, zoom.value());
    let (metadata, slots) = drape_metadata(&specs);
    let ranges = {
        let Some(Initialized(pattern)) =
            world.resources.get_mut::<Eventually<WgpuTileViewPattern>>()
        else {
            return Err(SystemError::Dependencies);
        };
        pattern
            .upload_extra_metadata(queue, &metadata)
            .map_err(|error| {
                tracing::error!(%error, "unable to upload drape metadata");
                SystemError::Setup
            })?
    };
    let phase = build_drape_phase(specs, &slots, &ranges, zoom, background_clear_color(style));

    let gpu_view_projection = view_state.gpu_view_projection();
    let skirt_length = 2.0 * std::f64::consts::PI * EARTH_RADIUS_METERS
        / 2_f64.powf(zoom.value().max(0.0))
        / SKIRT_DIVISOR;
    {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.ensure_scratch(device);
        let keep: HashSet<WorldTileCoords> = phase.targets.iter().map(|t| t.coords).collect();
        terrain.retain_drape_textures(&keep);
        let mut uniforms = Vec::with_capacity(phase.targets.len());
        let mut sources = Vec::with_capacity(phase.targets.len());
        for target in &phase.targets {
            terrain.ensure_drape_texture(device, target.coords);
            let dem_coords = loaded_dem_tile(target.coords, &dem, terrain);
            let Some(block) = tile_uniforms(
                target.coords,
                dem_coords,
                terrain,
                &dem,
                &gpu_view_projection
                    .to_model_view_projection(target.coords.transform_for_zoom(zoom))
                    .downcast(),
                skirt_length as f32,
            ) else {
                continue;
            };
            uniforms.push(block);
            sources.push((dem_coords, target.coords));
        }
        let written = terrain.write_uniforms(queue, &uniforms);
        tracing::debug!(
            targets = phase.targets.len(),
            masks = phase.targets.iter().map(|t| t.masks.len()).sum::<usize>(),
            layers = phase.targets.iter().map(|t| t.layers.len()).sum::<usize>(),
            metadata = metadata.len(),
            dem_hits = sources.iter().filter(|(dem, _)| dem.is_some()).count(),
            written,
            "terrain frame queued"
        );
        let draws = sources
            .iter()
            .take(written)
            .enumerate()
            .filter_map(|(index, (dem_coords, coords))| {
                let drape = terrain.drape_texture(*coords)?;
                Some(TerrainDraw {
                    bind_group: terrain.create_bind_group(
                        device,
                        terrain.dem_texture(*dem_coords),
                        drape,
                    ),
                    uniform_offset: (index as u64 * UNIFORM_STRIDE) as u32,
                })
            })
            .collect();
        terrain.set_draws(draws);
    }

    let drapeable: HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| is_drapeable(&layer.type_))
        .map(|layer| layer.id.as_str())
        .collect();
    if let Some(layer_phase) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layer_phase.retain(|item| !drapeable.contains(item.style_layer.as_str()));
    }
    let draw_after_layer_index = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "background")
        .map(|layer| layer.index)
        .max()
        .unwrap_or(0);
    world.resources.insert(phase);
    world.resources.insert(TerrainFrame {
        active: true,
        draw_after_layer_index,
    });
    Ok(())
}

/// Pairs every view tile with the source tiles that hold its data, without the screen path's
/// parent de-duplication: each drape texture needs its own copy of an ancestor's content.
fn select_targets(
    coords: impl Iterator<Item = WorldTileCoords>,
    world: &World,
) -> Vec<(WorldTileCoords, Vec<WorldTileCoords>)> {
    let Some(sources) = world.resources.get::<ViewTileSources>() else {
        return Vec::new();
    };
    coords
        .filter(|coords| coords.build_quad_key().is_some())
        .map(|coords| {
            let shapes = if sources.has_tile(coords, world) {
                vec![coords]
            } else if let Some(parent) = sources.get_available_parent(coords, world) {
                vec![parent]
            } else {
                sources
                    .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                    .unwrap_or_default()
            };
            (coords, shapes)
        })
        .collect()
}

fn collect_layer_specs(
    targets: Vec<(WorldTileCoords, Vec<WorldTileCoords>)>,
    style: &Style,
    world: &World,
    zoom: f64,
) -> Vec<TargetSpec> {
    let vector = world.resources.get::<Eventually<VectorBufferPool>>();
    let raster = world.resources.get::<Eventually<RasterResources>>();
    let raster_layers: Vec<(String, u32)> = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "raster" && layer.is_visible_at(zoom))
        .map(|layer| (layer.id.clone(), layer.index))
        .collect();
    targets
        .into_iter()
        .map(|(coords, shapes)| TargetSpec {
            coords,
            shapes: shapes
                .into_iter()
                .map(|source| {
                    let vector_layers = match vector {
                        Some(Initialized(pool)) => pool
                            .index()
                            .get_layers(source)
                            .into_iter()
                            .flatten()
                            .filter(|entry| {
                                entry.style_layer.is_visible_at(zoom)
                                    && is_drapeable(&entry.style_layer.type_)
                            })
                            .map(|entry| VectorLayerSpec {
                                id: entry.style_layer.id.clone(),
                                index: entry.style_layer.index,
                                is_line: entry.style_layer.type_ == "line",
                                coords: entry.coords,
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    let has_raster = matches!(raster, Some(Initialized(resources))
                        if resources.get_bound_texture(&source).is_some());
                    ShapeSpec {
                        source,
                        vector_layers,
                        raster_layers: if has_raster {
                            raster_layers.clone()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
        })
        .collect()
}

/// Instance metadata placing each source shape inside its target's drape texture.
fn drape_metadata(specs: &[TargetSpec]) -> (Vec<ShaderTileMetadata>, Vec<Option<usize>>) {
    let mut metadata = Vec::new();
    let mut slots = Vec::new();
    for spec in specs {
        let texture_zoom = Zoom::new(
            f64::from(u8::from(spec.coords.z)) + (f64::from(DRAPE_SIZE) / TILE_SIZE).log2(),
        );
        for shape in &spec.shapes {
            let transform = drape_transform(spec.coords, shape.source)
                .and_then(|transform| transform.cast::<f32>());
            let Some(transform) = transform else {
                slots.push(None);
                continue;
            };
            slots.push(Some(metadata.len()));
            metadata.push(ShaderTileMetadata {
                transform: transform.into(),
                zoom_factor: texture_zoom.scale_to_tile(&shape.source) as f32,
                viewport_width: DRAPE_SIZE as f32,
                viewport_height: DRAPE_SIZE as f32,
                tile_mercator_coords: tile_mercator_coordinates(
                    shape.source.into_tile(TileAddressingScheme::XYZ),
                )
                .into(),
                clip_antimeridian: 0,
            });
        }
    }
    (metadata, slots)
}

fn build_drape_phase(
    specs: Vec<TargetSpec>,
    slots: &[Option<usize>],
    ranges: &[std::ops::Range<wgpu::BufferAddress>],
    zoom: Zoom,
    clear_color: wgpu::Color,
) -> DrapePhase {
    let mut phase = DrapePhase::default();
    let mut slot = slots.iter();
    for spec in specs {
        let mut target = DrapeTarget {
            coords: spec.coords,
            clear_color,
            masks: Vec::new(),
            layers: Vec::new(),
        };
        for shape in spec.shapes {
            let Some(Some(index)) = slot.next() else {
                continue;
            };
            let Some(range) = ranges.get(*index) else {
                continue;
            };
            let source_shape = TileShape::with_buffer_range(shape.source, zoom, range.clone());
            target.masks.push(TileMaskItem {
                draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                source_shape: source_shape.clone(),
                generate_borders: false,
                projection: ProjectionBinding::Flat,
            });
            for layer in shape.vector_layers {
                let draw_function: Box<dyn Draw<LayerItem>> = if layer.is_line {
                    Box::new(DrawState::<LayerItem, DrawLineTiles>::new())
                } else {
                    Box::new(DrawState::<LayerItem, DrawVectorTiles>::new())
                };
                target.layers.push(LayerItem {
                    draw_function,
                    index: layer.index,
                    is_line: layer.is_line,
                    generate_borders: false,
                    style_layer: layer.id,
                    tile: Tile {
                        coords: layer.coords,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                });
            }
            for (id, index) in shape.raster_layers {
                target.layers.push(LayerItem {
                    draw_function: Box::new(DrawState::<LayerItem, DrawRasterTiles>::new()),
                    index,
                    is_line: false,
                    generate_borders: false,
                    style_layer: id,
                    tile: Tile {
                        coords: shape.source,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                });
            }
        }
        target.layers.sort_by_key(|item| item.index);
        phase.targets.push(target);
    }
    phase
}

/// The nearest uploaded DEM tile at or above the DEM zoom of a view tile.
fn loaded_dem_tile(
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

fn tile_uniforms(
    coords: WorldTileCoords,
    dem_coords: Option<WorldTileCoords>,
    terrain: &TerrainResources,
    dem: &DemSource,
    transform: &Matrix4<f32>,
    skirt_length: f32,
) -> Option<TerrainTileUniforms> {
    let (dem_matrix, dem_unpack, dem_dim) = match dem_coords {
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
        tile_mercator_coords: tile_mercator_coordinates(
            coords.into_tile(TileAddressingScheme::XYZ),
        )
        .into(),
        dem_unpack,
        dem_dim,
        exaggeration: dem.exaggeration,
        skirt_length,
        padding: 0.0,
    })
}

/// Color the drape textures start from: the constant background paint, or transparent.
fn background_clear_color(style: &Style) -> wgpu::Color {
    style
        .layers
        .iter()
        .find(|layer| layer.type_ == "background")
        .and_then(|layer| layer.paint.as_ref()?.get_color())
        .map(|color| wgpu::Color {
            r: f64::from(color.color.r),
            g: f64::from(color.color.g),
            b: f64::from(color.color.b),
            a: f64::from(color.alpha),
        })
        .unwrap_or(wgpu::Color::TRANSPARENT)
}

//! Builds the drape targets and terrain draws for the current frame.

use std::collections::HashSet;

use cgmath::{Matrix4, SquareMatrix, Vector3};

use crate::{
    context::MapContext,
    coords::{WorldTileCoords, Zoom, EXTENT, TILE_SIZE},
    hillshade::render_commands::DrawDemTiles,
    projection::renderer_data::tile_mercator_coordinates,
    raster::{render_commands::DrawRasterTiles, resource::RasterResources},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::{raster_source_regions, view_region_for_projection},
        render_commands::DrawMasks,
        render_phase::{Draw, DrawState, LayerItem, ProjectionBinding, RenderPhase, TileMaskItem},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{TileShape, WgpuTileViewPattern, DEFAULT_TILE_SIZE},
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
        drape_cache::{fingerprint, SourceRevisions},
        drape_targets::{collect_layer_specs, is_drapeable, select_targets, TargetSpec},
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

/// Metadata slots of the shapes of one target, `None` where a shape gets no slot.
type TargetSlots = Vec<Option<usize>>;

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

    let raster_coverings = raster_source_regions(style, view_state, world, ViewStatePadding::Tight)
        .map_err(|error| {
            tracing::error!(%error, "unable to select terrain raster tiles");
            SystemError::Setup
        })?;
    let targets = select_targets(view_region.iter(), world, &raster_coverings);
    let specs = collect_layer_specs(targets, style, world, zoom.value());
    let revisions = source_revisions(world);
    let clear_color = background_clear_color(style);
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => return Err(SystemError::Dependencies),
    };

    // Textures whose fingerprint is unchanged keep their content; only the rest are redrawn.
    let redraw: Vec<bool> = {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.ensure_scratch(device);
        let keep: HashSet<WorldTileCoords> = specs.iter().map(|spec| spec.coords).collect();
        terrain.retain_drapes(&keep);
        specs
            .iter()
            .map(|spec| {
                terrain.acquire_drape(
                    device,
                    spec.coords,
                    fingerprint(spec, revisions, clear_color),
                )
            })
            .collect()
    };
    let (metadata, slots) = drape_metadata(&specs, &redraw, capacity);
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
    let phase = build_drape_phase(&specs, &redraw, &slots, &ranges, zoom, clear_color);

    let gpu_view_projection = view_state.gpu_view_projection();
    let skirt_length = view_state.body().circumference_meters()
        / 2_f64.powf(zoom.value().max(0.0))
        / SKIRT_DIVISOR;
    {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        let mut uniforms = Vec::with_capacity(specs.len());
        let mut sources = Vec::with_capacity(specs.len());
        for spec in &specs {
            let dem_coords = loaded_dem_tile(spec.coords, &dem, terrain);
            let Some(block) = tile_uniforms(
                spec.coords,
                dem_coords,
                terrain,
                &dem,
                &gpu_view_projection
                    .to_model_view_projection(spec.coords.transform_for_zoom(zoom))
                    .downcast(),
                skirt_length as f32,
            ) else {
                continue;
            };
            uniforms.push(block);
            sources.push((dem_coords, spec.coords));
        }
        let written = terrain.write_uniforms(queue, &uniforms);
        tracing::debug!(
            targets = specs.len(),
            redrawn = phase.targets.len(),
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

/// Revisions of the sources drawn into drape textures.
fn source_revisions(world: &World) -> SourceRevisions {
    SourceRevisions {
        raster: match world.resources.get::<Eventually<RasterResources>>() {
            Some(Initialized(resources)) => resources.revision(),
            _ => 0,
        },
        vector: match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => pool.revision(),
            _ => 0,
        },
    }
}

/// Instance metadata placing each redrawn target's source shapes inside its drape texture.
///
/// Shapes past `capacity` get no slot: a view falling back to many small child tiles can ask
/// for more than the metadata buffer holds, and those shapes wait for their own tiles.
fn drape_metadata(
    specs: &[TargetSpec],
    redraw: &[bool],
    capacity: usize,
) -> (Vec<ShaderTileMetadata>, Vec<TargetSlots>) {
    let mut metadata = Vec::new();
    let mut slots = Vec::with_capacity(specs.len());
    let mut skipped = 0_usize;
    for (spec, redraw) in specs.iter().zip(redraw) {
        let texture_zoom = Zoom::new(
            f64::from(u8::from(spec.coords.z)) + (f64::from(DRAPE_SIZE) / TILE_SIZE).log2(),
        );
        let mut target_slots = Vec::with_capacity(spec.shapes.len());
        for shape in &spec.shapes {
            let transform = redraw
                .then(|| drape_transform(spec.coords, shape.source))
                .flatten()
                .and_then(|transform| transform.cast::<f32>());
            let Some(transform) = transform else {
                target_slots.push(None);
                continue;
            };
            if metadata.len() >= capacity {
                target_slots.push(None);
                skipped += 1;
                continue;
            }
            target_slots.push(Some(metadata.len()));
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
        slots.push(target_slots);
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            capacity,
            "drape shapes exceed the metadata buffer; some tiles drape without them this frame"
        );
    }
    (metadata, slots)
}

fn build_drape_phase(
    specs: &[TargetSpec],
    redraw: &[bool],
    slots: &[TargetSlots],
    ranges: &[std::ops::Range<wgpu::BufferAddress>],
    zoom: Zoom,
    clear_color: wgpu::Color,
) -> DrapePhase {
    let mut phase = DrapePhase::default();
    for ((spec, redraw), target_slots) in specs.iter().zip(redraw).zip(slots) {
        if !redraw {
            continue;
        }
        let mut target = DrapeTarget {
            coords: spec.coords,
            clear_color,
            masks: Vec::new(),
            layers: Vec::new(),
        };
        for (shape, slot) in spec.shapes.iter().zip(target_slots) {
            let Some(range) = slot.and_then(|index| ranges.get(index)) else {
                continue;
            };
            let source_shape = TileShape::with_buffer_range(shape.source, zoom, range.clone());
            target.masks.push(TileMaskItem {
                draw_function: Box::new(DrawState::<TileMaskItem, DrawMasks>::new()),
                source_shape: source_shape.clone(),
                generate_borders: false,
                projection: ProjectionBinding::Flat,
            });
            for layer in &shape.vector_layers {
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
                    style_layer: layer.id.clone(),
                    tile: Tile {
                        coords: layer.coords,
                    },
                    source_shape: source_shape.clone(),
                    projection: ProjectionBinding::Flat,
                });
            }
            for (id, index, dem) in &shape.raster_layers {
                let draw_function: Box<dyn Draw<LayerItem>> = if *dem {
                    Box::new(DrawState::<LayerItem, DrawDemTiles>::new())
                } else {
                    Box::new(DrawState::<LayerItem, DrawRasterTiles>::new())
                };
                target.layers.push(LayerItem {
                    draw_function,
                    index: *index,
                    is_line: false,
                    generate_borders: false,
                    style_layer: id.clone(),
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
        .find(|layer| layer.type_ == "background" && !layer.is_hidden())
        .and_then(|layer| layer.paint.as_ref()?.get_color())
        .map(|color| wgpu::Color {
            r: f64::from(color.color.r),
            g: f64::from(color.color.g),
            b: f64::from(color.color.b),
            a: f64::from(color.alpha),
        })
        .unwrap_or(wgpu::Color::TRANSPARENT)
}

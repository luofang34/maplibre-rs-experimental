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
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    style::{source::TileAddressingScheme, Style},
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
        world::World,
    },
    terrain::{
        drape_cache::{fingerprint, DrapeState, SourceContent},
        drape_targets::{collect_layer_specs, is_drapeable, select_targets, TargetSpec},
        request_system::dem_tile_coords,
        resources::TerrainFog,
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
/// Drape textures drawn in one frame. Every level of a drape's mip chain is a render pass,
/// and the Metal backend closes a command buffer per pass that stays open until the frame is
/// submitted, so a burst of arriving tiles must not open more than the queue holds; it also
/// keeps the frame's GPU time bounded while the rest wait a frame.
const MAX_DRAPES_PER_FRAME: usize = 24;
/// Drape textures drawn per frame while a host's eye drives the map. A flight lands hundreds
/// of tiles within a few frames, and a display's frame budget holds only a few drapes.
const EYE_DRAPES_PER_FRAME: usize = 8;

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
    let clear_color = background_clear_color(style);
    let prints: Vec<u64> = {
        let content = loaded_content(world);
        specs
            .iter()
            .map(|spec| fingerprint(spec, &content, clear_color))
            .collect()
    };
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => return Err(SystemError::Dependencies),
    };

    // Textures whose fingerprint is unchanged keep their content; only the rest are redrawn,
    // and only so many per frame.
    let (redraw, hidden): (Vec<bool>, Vec<bool>) = {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.ensure_scratch(device);
        let keep: HashSet<WorldTileCoords> = specs.iter().map(|spec| spec.coords).collect();
        terrain.retain_drapes(&keep);
        let states: Vec<DrapeState> = specs
            .iter()
            .zip(&prints)
            .map(|(spec, print)| terrain.acquire_drape(device, spec.coords, *print))
            .collect();
        let budget = if view_state.has_external_view() {
            EYE_DRAPES_PER_FRAME
        } else {
            MAX_DRAPES_PER_FRAME
        };
        let redraw = budget_redraws(&states, budget);
        for ((spec, state), drawn) in specs.iter().zip(&states).zip(&redraw) {
            if *state != DrapeState::Unchanged && !drawn {
                terrain.defer_drape(spec.coords);
            }
        }
        let hidden = awaiting_first_draw(&states, &redraw);
        (redraw, hidden)
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
    let fog = terrain_fog(style, view_state);
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
        for (spec, hidden) in specs.iter().zip(&hidden) {
            if *hidden {
                continue;
            }
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
                &fog,
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
                    coords: *coords,
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

/// Which of the acquired drapes to draw this frame, at most `budget`: tiles never drawn
/// first, since a reused texture shows another tile's content until they are, then tiles
/// whose content changed.
fn budget_redraws(states: &[DrapeState], budget: usize) -> Vec<bool> {
    let mut redraw = vec![false; states.len()];
    let mut drawn = 0;
    for wanted in [DrapeState::New, DrapeState::Changed] {
        for (index, state) in states.iter().enumerate() {
            if drawn >= budget {
                return redraw;
            }
            if *state == wanted {
                redraw[index] = true;
                drawn += 1;
            }
        }
    }
    redraw
}

/// Tiles whose texture holds no content of their own yet. A new drape the budget deferred
/// would show whatever tile last used the texture, so the tile is left out of this frame's
/// draws and appears once drawn.
fn awaiting_first_draw(states: &[DrapeState], redraw: &[bool]) -> Vec<bool> {
    states
        .iter()
        .zip(redraw)
        .map(|(state, drawn)| *state == DrapeState::New && !drawn)
        .collect()
}

/// The GPU content of source tiles, read from the vector pool and the raster textures.
struct LoadedContent<'a> {
    pool: Option<&'a VectorBufferPool>,
    raster: Option<&'a RasterResources>,
}

impl SourceContent for LoadedContent<'_> {
    fn vector_layer_loaded(&self, coords: WorldTileCoords, layer_id: &str) -> bool {
        self.pool.is_some_and(|pool| {
            pool.get_loaded_style_layers_at(coords)
                .is_some_and(|layers| layers.contains(layer_id))
        })
    }

    fn raster_loaded(&self, coords: WorldTileCoords) -> bool {
        self.raster
            .is_some_and(|raster| raster.get_bound_texture(&coords).is_some())
    }
}

fn loaded_content(world: &World) -> LoadedContent<'_> {
    LoadedContent {
        pool: match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => Some(pool),
            _ => None,
        },
        raster: match world.resources.get::<Eventually<RasterResources>>() {
            Some(Initialized(raster)) => Some(raster),
            _ => None,
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
    fog: &TerrainFog,
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
        fog_color: fog.fog_color,
        horizon_color: fog.horizon_color,
        fog_range: [fog.near, fog.far, fog.ground_blend, fog.horizon_blend],
        fog_opacity: [fog.opacity, if fog.globe { 1.0 } else { 0.0 }, 0.0, 0.0],
    })
}

/// The fog of the frame from the style's sky and the view, as GL JS `terrainUniformValues`.
fn terrain_fog(style: &Style, view_state: &ViewState) -> TerrainFog {
    let Some(sky) = &style.sky else {
        return TerrainFog::default();
    };
    let colors = sky.colors_at(view_state.zoom().value());
    let (near, far) = view_state.fog_depth_range();
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

#[cfg(test)]
mod tests {
    use super::{awaiting_first_draw, budget_redraws, DrapeState};

    #[test]
    fn a_frame_draws_new_tiles_first_and_at_most_the_budget() {
        use DrapeState::{Changed, New, Unchanged};
        let states = [Changed, New, Unchanged, New, Changed, New];
        assert_eq!(
            budget_redraws(&states, 4),
            [true, true, false, true, false, true],
            "three new tiles and the first changed one"
        );
        assert_eq!(
            budget_redraws(&states, 10),
            [true, true, false, true, true, true],
            "everything but the unchanged tile fits"
        );
        assert_eq!(budget_redraws(&states, 0), [false; 6]);
    }

    #[test]
    fn a_new_tile_the_budget_deferred_is_not_drawn_with_borrowed_content() {
        use DrapeState::{Changed, New, Unchanged};
        let states = [New, Changed, New, Unchanged, New];
        let redraw = budget_redraws(&states, 2);
        assert_eq!(redraw, [true, false, true, false, false]);
        assert_eq!(
            awaiting_first_draw(&states, &redraw),
            [false, false, false, false, true],
            "only the undrawn new tile waits; a changed tile keeps showing its last content"
        );
    }
}

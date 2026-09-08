//! Builds the drape targets and terrain draws for the current frame.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        eye_covering::{drawn_covering, EyeInFrame},
        memory_budget::MemoryBudget,
        render_phase::{LayerItem, RenderPhase},
        tile_view_pattern::{WgpuTileViewPattern, DEFAULT_TILE_SIZE},
        view_state::ViewState,
        Renderer,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
    terrain::{
        drape_cache::{fingerprint, DrapeState, SourceContent},
        drape_targets::{collect_layer_specs, is_drapeable, select_targets, TargetSpec},
        resources::{TerrainDraw, TerrainResources, UNIFORM_STRIDE},
        source::{dem_source, DemSource},
        DrapePhase, TerrainFrame,
    },
    vector::{geometry_uploaded, VectorBufferPool},
};
mod covering;
mod drape_phase;
mod uniforms;
use drape_phase::{background_clear_color, build_drape_phase, drape_metadata};
use uniforms::{present_ancestors, TerrainEyeFrame};

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

pub fn queue_system(
    MapContext {
        style,
        view_state,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if EyeInFrame::reuses_content(world) {
        return uniforms::replay(world, style, view_state, queue);
    }
    world.resources.insert(TerrainEyeFrame::default());
    let Some(dem) = dem_source(style) else {
        world.resources.insert(DrapePhase::default());
        world.resources.insert(TerrainFrame::default());
        return Ok(());
    };
    let specs = target_specs(style, view_state, world)?;
    let PreparedDrapes {
        redraw,
        sources,
        clear_color,
    } = prepare_drapes(&specs, style, view_state, world, device)?;
    let phase = encode_drapes(
        &specs,
        &redraw,
        clear_color,
        world,
        view_state.zoom(),
        queue,
    )?;
    queue_tiles(
        world,
        style,
        view_state,
        device,
        queue,
        &dem,
        (&specs, &sources),
    )?;
    hide_draped_layers(world, style);
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

fn hide_draped_layers(world: &mut World, style: &Style) {
    let drapeable: HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| {
            is_drapeable(&layer.type_) && crate::vector::structures::kind(layer).is_none()
        })
        .map(|layer| layer.id.as_str())
        .collect();
    if let Some(layer_phase) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layer_phase.retain(|item| !drapeable.contains(item.style_layer.as_str()));
    }
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
/// would show whatever tile last used the texture, so its surface uses an ancestor or the
/// background color until its own texture is drawn.
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

#[cfg(test)]
mod tests;

fn acquire_drapes(
    specs: &[TargetSpec],
    prints: &[u64],
    ready: &[bool],
    memory: MemoryBudget,
    external: bool,
    terrain: &mut TerrainResources,
    device: &wgpu::Device,
) -> (Vec<bool>, Vec<Option<WorldTileCoords>>) {
    // Ancestors that still hold a texture stand in for tiles not drawn yet, so they stay
    // until every tile under them is drawn.
    let mut keep: HashSet<WorldTileCoords> = specs.iter().map(|spec| spec.coords).collect();
    for spec in specs {
        keep.extend(present_ancestors(spec.coords, terrain));
    }
    terrain.retain_drapes(&keep);
    // Unready tiles keep their last valid texture or a background surface.
    if memory.is_tight() {
        terrain.shed_spare_drapes();
    }
    // Targets come nearest first, so when textures run out it is the far tiles that
    // draw with an ancestor's drape.
    let states: Vec<DrapeState> = specs
        .iter()
        .zip(prints)
        .zip(ready)
        .map(|((spec, print), ready)| {
            if *ready {
                let may_create = memory.allows_drape_texture(terrain.drape_texture_total());
                terrain.acquire_drape(device, spec.coords, *print, may_create)
            } else {
                DrapeState::Unchanged
            }
        })
        .collect();
    let budget = if external {
        EYE_DRAPES_PER_FRAME
    } else {
        MAX_DRAPES_PER_FRAME
    };
    let redraw = budget_redraws(&states, budget);
    for ((spec, state), drawn) in specs.iter().zip(&states).zip(&redraw) {
        if !matches!(state, DrapeState::Unchanged | DrapeState::Withheld) && !drawn {
            terrain.defer_drape(spec.coords, *state);
        }
    }
    let sources = drape_sources(specs, &states, &redraw, ready, terrain);
    (redraw, sources)
}

fn drape_sources(
    specs: &[TargetSpec],
    states: &[DrapeState],
    redraw: &[bool],
    ready: &[bool],
    terrain: &TerrainResources,
) -> Vec<Option<WorldTileCoords>> {
    let hidden: Vec<bool> = awaiting_first_draw(states, redraw)
        .into_iter()
        .zip(ready)
        .zip(states)
        .map(|((hidden, ready), state)| hidden || !ready || *state == DrapeState::Withheld)
        .collect();
    let undrawn: HashSet<WorldTileCoords> = specs
        .iter()
        .zip(&hidden)
        .filter(|(_, hidden)| **hidden)
        .map(|(spec, _)| spec.coords)
        .collect();
    specs
        .iter()
        .zip(&hidden)
        .map(|(spec, hidden)| {
            if *hidden && terrain.drape_texture(spec.coords).is_none() {
                present_ancestors(spec.coords, terrain)
                    .into_iter()
                    .find(|ancestor| !undrawn.contains(ancestor))
            } else {
                Some(spec.coords)
            }
        })
        .collect()
}

fn queue_tiles(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dem: &DemSource,
    targets: (&[TargetSpec], &[Option<WorldTileCoords>]),
) -> SystemResult {
    {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        let uniforms::PreparedTerrain { uniforms, sources } =
            uniforms::prepare_tiles(targets, style, view_state, terrain, dem);
        let written = terrain.write_uniforms(queue, &uniforms);
        tracing::debug!(
            targets = targets.0.len(),
            dem_hits = sources.iter().filter(|(dem, _, _)| dem.is_some()).count(),
            written,
            "terrain frame queued"
        );
        let draws = sources
            .iter()
            .take(written)
            .enumerate()
            .filter_map(|(index, (dem_coords, coords, drape_source))| {
                Some(TerrainDraw {
                    coords: *coords,
                    bind_group: terrain.tile_bind_group(device, *dem_coords, *drape_source)?,
                    uniform_offset: (index as u64 * UNIFORM_STRIDE) as u32,
                })
            })
            .collect();
        terrain.set_draws(draws);
        let kept = sources
            .iter()
            .zip(uniforms)
            .take(written)
            .map(|((_, coords, _), uniform)| (*coords, uniform))
            .collect();
        world.resources.insert(TerrainEyeFrame(kept));
    }

    Ok(())
}

fn target_specs(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
) -> Result<Vec<TargetSpec>, SystemError> {
    let zoom = view_state.zoom();
    let (view_region, raster_coverings) =
        drawn_covering(style, view_state, world, zoom.zoom_level(DEFAULT_TILE_SIZE)).map_err(
            |error| {
                tracing::error!(%error, "unable to select terrain tiles");
                SystemError::Setup
            },
        )?;
    let Some(view_region) = view_region else {
        return Ok(Vec::new());
    };
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    let tiles: Vec<_> = if view_state.has_external_view() {
        covering::for_frame(
            world,
            view_region.iter().collect(),
            memory.drape_textures_allowed(),
        )
    } else {
        view_region.iter().collect()
    };
    world
        .resources
        .insert(crate::terrain::request_system::DrapeRequests(tiles.clone()));
    let targets = select_targets(tiles.into_iter(), world, &raster_coverings);
    Ok(collect_layer_specs(targets, style, world, zoom.value()))
}

struct PreparedDrapes {
    redraw: Vec<bool>,
    sources: Vec<Option<WorldTileCoords>>,
    clear_color: wgpu::Color,
}

fn prepare_drapes(
    specs: &[TargetSpec],
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    device: &wgpu::Device,
) -> Result<PreparedDrapes, SystemError> {
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    let clear_color = background_clear_color(style);
    let prints: Vec<u64> = {
        let content = loaded_content(world);
        specs
            .iter()
            .map(|spec| fingerprint(spec, &content, clear_color, view_state.zoom().value()))
            .collect()
    };
    // A tile whose vector sources are finished but not yet in the buffer pool would drape
    // blank; it shows an ancestor's drape until they are.
    let ready: Vec<bool> = specs
        .iter()
        .map(|spec| {
            !spec.shapes.is_empty()
                && spec.shapes.iter().all(|shape| {
                    !shape.raster_layers.is_empty() || geometry_uploaded(shape.source, world)
                })
        })
        .collect();
    let (redraw, drape_sources) = {
        let Some(Initialized(terrain)) = world.resources.get_mut::<Eventually<TerrainResources>>()
        else {
            return Err(SystemError::Dependencies);
        };
        terrain.ensure_scratch(device);
        acquire_drapes(
            specs,
            &prints,
            &ready,
            memory,
            view_state.has_external_view(),
            terrain,
            device,
        )
    };
    Ok(PreparedDrapes {
        redraw,
        sources: drape_sources,
        clear_color,
    })
}

fn encode_drapes(
    specs: &[TargetSpec],
    redraw: &[bool],
    clear_color: wgpu::Color,
    world: &mut World,
    zoom: crate::coords::Zoom,
    queue: &wgpu::Queue,
) -> Result<DrapePhase, SystemError> {
    let capacity = match world.resources.get::<Eventually<WgpuTileViewPattern>>() {
        Some(Initialized(pattern)) => pattern.remaining_metadata_capacity(),
        _ => return Err(SystemError::Dependencies),
    };

    let (metadata, slots) = drape_metadata(specs, redraw, capacity, zoom);
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
    Ok(build_drape_phase(
        specs,
        redraw,
        &slots,
        &ranges,
        zoom,
        clear_color,
    ))
}

//! Evaluates visible background paint and queues sky and atmosphere draws.

use cgmath::InnerSpace;

pub(crate) use super::buffers::globe_tiles;
pub use super::buffers::{BackgroundBuffers, BackgroundGpuBuffers};
use super::{
    buffers::{
        background_color, fill_globe_tiles, globe_tile_metadata_with_vp, sky_metadata_with_camera,
        world_rows,
    },
    globe_sky::{draws_globe_sky, globe_sky_metadata, globe_sky_metadata_with_camera},
    render_commands::{
        DrawAtmosphere, DrawBackground, DrawBackgroundPattern, DrawGlobeBackground,
        DrawGlobeBackgroundPattern, DrawSky,
    },
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TranslucentItem},
        resolved_view::ResolvedFrameView,
        shaders::{AtmosphereLayerMetadata, BackgroundLayerMetadata},
        view_state::ViewState,
    },
    style::light::LightSpecification,
    tcs::system::{SystemError, SystemResult},
};

/// Returns `true` when an unclipped flat quad is guaranteed to pass every `background.fragment.wgsl` check.
pub(crate) fn flat_viewport_covered(view_state: &ViewState, edges: bool, frame: [f32; 2]) -> bool {
    if !edges || frame[0] <= 0.0 || frame[1] <= 0.0 {
        return false;
    }
    let horizon = view_state.horizon_line().to_shader();
    let [row_num, row_den] = world_rows(view_state, edges);
    let logical_h = view_state.height() as f32;
    let scale = logical_h / frame[1].max(1.0);
    for (px, py, nx, ny) in [
        (0.0_f32, 0.0_f32, -1.0_f32, 1.0_f32),
        (frame[0], 0.0, 1.0, 1.0),
        (0.0, frame[1], -1.0, -1.0),
        (frame[0], frame[1], 1.0, -1.0),
    ] {
        let x = px * scale;
        let y = logical_h - py * scale;
        let distance = (y - horizon[1]) * horizon[3] + (x - horizon[0]) * horizon[2];
        if !distance.is_finite() || distance > -1e-3 {
            return false;
        }
        let denom = row_den[0] * nx + row_den[1] * ny + row_den[2];
        if !denom.is_finite() || denom <= 1e-9 {
            return false;
        }
        let row = (row_num[0] * nx + row_num[1] * ny + row_num[2]) / denom;
        if !row.is_finite() || row < row_num[3] + 1e-3 || row > row_den[3] - 1e-3 {
            return false;
        }
    }
    true
}

const SKY_SPACE_CUTOFF_RATIO: f64 = 10.0 * (100_000.0 / 6_371_008.8);

fn atmosphere_blend_state(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> (f32, bool) {
    let zoom = view_state.zoom().value();
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification.projection_type.globe_transition(zoom)
    });
    let sky_blend = style
        .sky
        .as_ref()
        .map_or(0.0, |sky| sky.atmosphere_blend_at_zoom(zoom));
    let host_blend = f32::from(u8::from(
        world
            .resources
            .get::<crate::render::SceneLighting>()
            .is_some_and(|l| l.atmosphere),
    ));
    let sky_faded = if sky_blend > 0.0 {
        let altitude_blend = ResolvedFrameView::globe_camera_for_frame(world, style, view_state)
            .map_or(0.0, |camera| {
                super::altitude::altitude_blend_for_camera(&camera)
            });
        sky_blend * altitude_blend
    } else {
        0.0
    };
    let over_globe = sky_faded > 0.0 && host_blend <= 0.0;
    (sky_faded.max(host_blend) * transition, over_globe)
}

/// How strongly the atmosphere is drawn this frame: as the style's `sky` blends it, or in full
/// where the host turned it on, faded with the projection's globe transition and camera altitude below 100 km.
pub fn atmosphere_blend(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> f32 {
    atmosphere_blend_state(style, view_state, world).0
}

/// Appends visible background, sky and atmosphere items and uploads their frame metadata.
/// Returns `Dependencies` if the required phases are absent, or `Setup` if atmosphere
/// camera or shader metadata cannot be constructed. Existing phase items are not cleared.
pub fn queue_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world.resources.get::<RenderPhase<LayerItem>>().is_none()
        || world
            .resources
            .get::<RenderPhase<TranslucentItem>>()
            .is_none()
    {
        return Err(SystemError::Dependencies);
    }
    let frame = {
        let size = renderer.state().surface().size();
        [size.width() as f32, size.height() as f32]
    };
    let projection_transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    let uses_globe = projection_transition > 0.0;
    let world_rows = world_rows(view_state, !uses_globe && style.terrain.is_none());
    let horizon = view_state.horizon_line().to_shader();
    let (atmosphere_blend, atmosphere_over_globe) =
        atmosphere_blend_state(style, view_state, world);
    // Without an atmosphere a full globe still has a sky where the host wants the frame
    // covered; with one, the atmosphere draws the sky. Standard map views curve and fade
    // the sky across the globe horizon in `sky.fragment.wgsl` as GL JS does.
    let external_or_free = view_state.has_external_view()
        || view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe;
    let globe_sky = draws_globe_sky(view_state, projection_transition, atmosphere_blend);
    let (sky_metadata, atmosphere_metadata) = {
        let cached_globe_camera = if uses_globe || atmosphere_blend > 0.0 {
            ResolvedFrameView::globe_camera_for_frame(world, style, view_state).ok()
        } else {
            None
        };
        let space_sky_culled = projection_transition >= 1.0
            && !globe_sky
            && cached_globe_camera.as_deref().is_some_and(|camera| {
                camera.globe_radius_pixels() > 0.0
                    && camera.camera_position().magnitude() - 1.0 >= SKY_SPACE_CUTOFF_RATIO
            });
        let sky_metadata = style
            .sky
            .as_ref()
            .filter(|_| {
                !space_sky_culled
                    && (!external_or_free
                        || projection_transition < 1.0
                        || view_state.opaque_environment()
                        || globe_sky)
            })
            .map(|sky| {
                globe_sky
                    .then(|| match cached_globe_camera.as_deref() {
                        Some(camera) => globe_sky_metadata_with_camera(sky, view_state, camera),
                        None => globe_sky_metadata(sky, view_state),
                    })
                    .flatten()
                    .unwrap_or_else(|| {
                        sky_metadata_with_camera(
                            sky,
                            view_state,
                            projection_transition,
                            frame[1],
                            cached_globe_camera.as_deref(),
                        )
                    })
            });
        let atmosphere_metadata = if atmosphere_blend > 0.0 {
            let owned_camera = cached_globe_camera
                .is_none()
                .then(|| ResolvedFrameView::globe_camera_for_frame(world, style, view_state))
                .transpose()
                .map_err(|error| {
                    tracing::error!(?error, "failed to build atmosphere globe camera");
                    SystemError::Setup
                })?;
            let Some(camera) = cached_globe_camera.as_deref().or(owned_camera.as_deref()) else {
                return Err(SystemError::Setup);
            };
            let default_light = LightSpecification::default();
            let light = style.light.as_ref().unwrap_or(&default_light);
            let scene_lighting = world
                .resources
                .get::<crate::render::SceneLighting>()
                .copied()
                .unwrap_or_default();
            AtmosphereLayerMetadata::from_view(
                camera,
                light,
                scene_lighting,
                view_state.zoom().value(),
                atmosphere_blend,
            )
            .map(|metadata| metadata.with_depth_z(if atmosphere_over_globe { 1.0 } else { 0.0 }))
            .map_err(|error| {
                tracing::error!(?error, "failed to build atmosphere shader metadata");
                SystemError::Setup
            })?
        } else {
            AtmosphereLayerMetadata::disabled()
        };
        (sky_metadata, atmosphere_metadata)
    };

    if let Some(gpu) = world
        .resources
        .get_mut::<super::pattern::BackgroundPatternGpu>()
    {
        gpu.write(&renderer.queue, view_state, frame);
    }
    world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
    let has_draws = {
        let Some((layer_item_phase, translucent_phase, gpu_buffers)) =
            world.resources.query_mut::<(
                &mut RenderPhase<LayerItem>,
                &mut RenderPhase<TranslucentItem>,
                &mut BackgroundGpuBuffers,
            )>()
        else {
            return Err(SystemError::Dependencies);
        };
        let scratch = &mut gpu_buffers.scratch;
        scratch.metadatas.clear();
        scratch.instances.clear();
        scratch.opaque_layers.clear();

        let mut background_index = 0;
        for layer in &style.layers {
            if layer.type_ != "background" || !layer.is_visible_at(view_state.zoom().value()) {
                continue;
            }
            // Under terrain the drape of each terrain tile carries the background, so nothing
            // paints outside the world.
            if style.terrain.is_some() && !uses_globe {
                continue;
            }
            // A layer that names an image it does not have draws nothing, not its colour.
            if crate::vector::pattern::names_missing_image(
                layer.paint.as_ref(),
                style,
                view_state.style_zoom().value(),
            ) {
                continue;
            }
            let is_patterned = layer.paint.as_ref().is_some_and(|paint| {
                crate::vector::pattern::pattern_name(paint, view_state.style_zoom().value())
                    .is_some()
            });
            let c = background_color(layer, view_state.zoom().value());
            if uses_globe && !is_patterned && c[3] <= 0.0 {
                continue;
            }
            background_index = background_index.max(layer.index);
            let z_index = layer.index as f32;
            let layer_id = layer.layer_id();
            if !is_patterned && c[3] >= 1.0 {
                scratch.opaque_layers.insert(layer_id);
            }
            scratch
                .instances
                .insert(layer_id, scratch.metadatas.len() as u32);
            scratch.metadatas.push(BackgroundLayerMetadata {
                color: c,
                z_index,
                padding: [0.0; 3],
                horizon,
                viewport: [
                    view_state.height() as f32,
                    f32::from(style.terrain.is_some()),
                    frame[0],
                    frame[1],
                ],
                world_rows,
            });

            let draw_function: &'static dyn crate::render::render_phase::Draw<LayerItem> =
                if uses_globe && is_patterned {
                    &DrawState::<LayerItem, DrawGlobeBackgroundPattern>::INSTANCE
                } else if uses_globe {
                    &DrawState::<LayerItem, DrawGlobeBackground>::INSTANCE
                } else if is_patterned {
                    &DrawState::<LayerItem, DrawBackgroundPattern>::INSTANCE
                } else {
                    &DrawState::<LayerItem, DrawBackground>::INSTANCE
                };
            layer_item_phase.add(LayerItem {
                projection: ProjectionBinding::View,
                draw_function,
                index: layer.index,
                generate_borders: false,
                style_layer: layer_id,
                entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
                source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
                mask_group: crate::render::render_phase::MaskGroup::Unmasked,

                // We provide a dummy tile for background.
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
                run: None,
            });
        }
        // The sky follows the background layers and precedes everything else, as GL JS draws it
        // under the map; the flat map covers it below the horizon.
        if sky_metadata.is_some() {
            layer_item_phase.add(LayerItem {
                projection: ProjectionBinding::View,
                draw_function: &DrawState::<LayerItem, DrawSky>::INSTANCE,
                index: background_index,
                generate_borders: false,
                style_layer: crate::style::StyleLayerId::SKY,
                entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
                source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
                mask_group: crate::render::render_phase::MaskGroup::Unmasked,
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
                run: None,
            });
        }
        if atmosphere_blend > 0.0 {
            translucent_phase.add(TranslucentItem {
                draw_function: &DrawState::<TranslucentItem, DrawAtmosphere>::INSTANCE,
                index: u32::MAX,
                style_layer: crate::style::StyleLayerId::ATMOSPHERE,
                entry_slot: crate::render::render_phase::UNSET_ENTRY_SLOT,
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
                source_shape: crate::render::tile_view_pattern::ShapeDrawData::default(),
            });
        }
        !scratch.metadatas.is_empty() || atmosphere_blend > 0.0 || sky_metadata.is_some()
    };

    if has_draws {
        let mut globe_tiles = std::mem::take(
            &mut world
                .resources
                .get_or_init_mut::<BackgroundGpuBuffers>()
                .scratch
                .globe_tiles,
        );
        if uses_globe {
            fill_globe_tiles(world, &mut globe_tiles);
        } else {
            globe_tiles.clear();
        }
        let gpu_vp = world
            .resources
            .get::<ResolvedFrameView>()
            .filter(|resolved| resolved.matches(style, view_state))
            .map(|resolved| resolved.gpu_view_projection)
            .unwrap_or_else(|| view_state.gpu_view_projection());

        let (buffer, tile_metadata_buffer, atmosphere_metadata_buffer, sky_metadata_buffer) = {
            let gpu_buffers = world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
            let scratch = &mut gpu_buffers.scratch;
            if scratch.metadatas.is_empty() {
                scratch.metadatas.push(BackgroundLayerMetadata {
                    color: [0.0; 4],
                    z_index: 0.0,
                    padding: [0.0; 3],
                    horizon,
                    viewport: [
                        view_state.height() as f32,
                        f32::from(style.terrain.is_some()),
                        frame[0],
                        frame[1],
                    ],
                    world_rows,
                });
            }
            scratch.tile_metadata.clear();
            scratch.tile_metadata.extend(
                globe_tiles
                    .iter()
                    .map(|coords| globe_tile_metadata_with_vp(view_state, &gpu_vp, *coords)),
            );
            let buffer = gpu_buffers
                .write_staged_metadata(&renderer.device, &renderer.queue)
                .clone();
            let tile_metadata_buffer = gpu_buffers
                .write_staged_tile_metadata(&renderer.device, &renderer.queue)
                .clone();
            let atmosphere_metadata_buffer = gpu_buffers
                .write_atmosphere(&renderer.device, &renderer.queue, &atmosphere_metadata)
                .clone();
            let sky_metadata_buffer = sky_metadata.as_ref().map(|sky| {
                gpu_buffers
                    .write_sky(&renderer.device, &renderer.queue, sky)
                    .clone()
            });
            (
                buffer,
                tile_metadata_buffer,
                atmosphere_metadata_buffer,
                sky_metadata_buffer,
            )
        };
        let flat_covers_viewport =
            !uses_globe && flat_viewport_covered(view_state, style.terrain.is_none(), frame);
        if let Some((gpu_buffers, existing)) = world
            .resources
            .query_mut::<(&mut BackgroundGpuBuffers, &mut BackgroundBuffers)>()
        {
            existing.metadata_buffer = buffer;
            existing.tile_metadata_buffer = tile_metadata_buffer;
            existing.atmosphere_metadata_buffer = atmosphere_metadata_buffer;
            existing.sky_metadata_buffer = sky_metadata_buffer;
            existing.flat_covers_viewport = flat_covers_viewport;
            std::mem::swap(&mut existing.instances, &mut gpu_buffers.scratch.instances);
            std::mem::swap(
                &mut existing.opaque_layers,
                &mut gpu_buffers.scratch.opaque_layers,
            );
            std::mem::swap(&mut existing.globe_tiles, &mut globe_tiles);
            gpu_buffers.scratch.globe_tiles = globe_tiles;
        } else {
            let (instances, opaque_layers) = {
                let gpu_buffers = world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
                (
                    std::mem::take(&mut gpu_buffers.scratch.instances),
                    std::mem::take(&mut gpu_buffers.scratch.opaque_layers),
                )
            };
            world.resources.insert(BackgroundBuffers {
                metadata_buffer: buffer,
                instances,
                tile_metadata_buffer,
                globe_tiles,
                atmosphere_metadata_buffer,
                sky_metadata_buffer,
                flat_covers_viewport,
                opaque_layers,
            });
        }
    }

    Ok(())
}

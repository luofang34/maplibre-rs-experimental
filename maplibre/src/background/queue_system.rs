//! Evaluates visible background paint and queues sky and atmosphere draws.

pub(crate) use super::buffers::globe_tiles;
pub use super::buffers::{BackgroundBuffers, BackgroundGpuBuffers};
use super::{
    buffers::{background_color, globe_tile_metadata, sky_metadata, world_rows},
    globe_sky::{draws_globe_sky, globe_sky_metadata},
    render_commands::{
        DrawAtmosphere, DrawBackground, DrawBackgroundPattern, DrawGlobeBackground,
        DrawGlobeBackgroundPattern, DrawSky,
    },
};
use crate::{
    context::MapContext,
    render::{
        projection::globe_camera_for_view,
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TranslucentItem},
        shaders::{AtmosphereLayerMetadata, BackgroundLayerMetadata, ShaderTileMetadata},
    },
    tcs::system::{SystemError, SystemResult},
};

/// How strongly the atmosphere is drawn this frame: as the style's `sky` blends it, or in full
/// where the host turned it on, faded with the projection's globe transition and camera altitude below 100 km.
pub fn atmosphere_blend(
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    world: &crate::tcs::world::World,
) -> f32 {
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
        let altitude_blend = globe_camera_for_view(view_state).map_or(0.0, |camera| {
            super::altitude::altitude_blend_for_camera(&camera)
        });
        sky_blend * altitude_blend
    } else {
        0.0
    };
    sky_faded.max(host_blend) * transition
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
    let mut metadatas = Vec::new();
    let frame = {
        let size = renderer.state().surface().size();
        [size.width() as f32, size.height() as f32]
    };
    let world_rows = world_rows(view_state, style.terrain.is_none());
    let mut instances = rustc_hash::FxHashMap::default();
    let projection_transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    let uses_globe = projection_transition > 0.0;
    let atmosphere_blend = atmosphere_blend(style, view_state, world);
    // Without an atmosphere a full globe still has a sky where the host wants the frame
    // covered; with one, the atmosphere draws the sky. Standard map views curve and fade
    // the sky across the globe horizon in `sky.fragment.wgsl` as GL JS does.
    let external_or_free = view_state.has_external_view()
        || view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe;
    let globe_sky = draws_globe_sky(view_state, projection_transition, atmosphere_blend);
    let sky_metadata = style
        .sky
        .as_ref()
        .filter(|_| {
            !external_or_free
                || projection_transition < 1.0
                || view_state.opaque_environment()
                || globe_sky
        })
        .map(|sky| {
            globe_sky
                .then(|| globe_sky_metadata(sky, view_state))
                .flatten()
                .unwrap_or_else(|| sky_metadata(sky, view_state, projection_transition, frame[1]))
        });

    if let Some(gpu) = world
        .resources
        .get_mut::<super::pattern::BackgroundPatternGpu>()
    {
        let size = renderer.state().surface().size();
        gpu.write(
            &renderer.queue,
            view_state,
            [size.width() as f32, size.height() as f32],
        );
    }
    {
        let Some((layer_item_phase, translucent_phase)) = world.resources.query_mut::<(
            &mut RenderPhase<LayerItem>,
            &mut RenderPhase<TranslucentItem>,
        )>() else {
            return Err(SystemError::Dependencies);
        };

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
            background_index = background_index.max(layer.index);
            let c = background_color(layer, view_state.zoom().value());
            let z_index = layer.index as f32;
            let layer_id = layer.layer_id();
            instances.insert(layer_id, metadatas.len() as u32);
            metadatas.push(BackgroundLayerMetadata {
                color: c,
                z_index,
                padding: [0.0; 3],
                horizon: view_state.horizon_line().to_shader(),
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
    }

    if !metadatas.is_empty() || atmosphere_blend > 0.0 || sky_metadata.is_some() {
        if metadatas.is_empty() {
            metadatas.push(BackgroundLayerMetadata {
                color: [0.0; 4],
                z_index: 0.0,
                padding: [0.0; 3],
                horizon: view_state.horizon_line().to_shader(),
                viewport: [
                    view_state.height() as f32,
                    f32::from(style.terrain.is_some()),
                    frame[0],
                    frame[1],
                ],
                world_rows,
            });
        }
        let globe_tiles = if uses_globe {
            globe_tiles(world)
        } else {
            Vec::new()
        };
        let tile_metadata: Vec<ShaderTileMetadata> = globe_tiles
            .iter()
            .map(|coords| globe_tile_metadata(view_state, *coords))
            .collect();
        let atmosphere_metadata = if atmosphere_blend > 0.0 {
            let camera = globe_camera_for_view(view_state).map_err(|error| {
                tracing::error!(?error, "failed to build atmosphere globe camera");
                SystemError::Setup
            })?;
            let light = style.light.clone().unwrap_or_default();
            let scene_lighting = world
                .resources
                .get::<crate::render::SceneLighting>()
                .copied()
                .unwrap_or_default();
            AtmosphereLayerMetadata::from_view(
                &camera,
                &light,
                scene_lighting,
                view_state.zoom().value(),
                atmosphere_blend,
            )
            .map_err(|error| {
                tracing::error!(?error, "failed to build atmosphere shader metadata");
                SystemError::Setup
            })?
        } else {
            AtmosphereLayerMetadata::disabled()
        };

        let (buffer, tile_metadata_buffer, atmosphere_metadata_buffer, sky_metadata_buffer) = {
            let gpu_buffers = world.resources.get_or_init_mut::<BackgroundGpuBuffers>();
            let buffer = gpu_buffers
                .write_metadata(&renderer.device, &renderer.queue, &metadatas)
                .clone();
            let tile_metadata_buffer = gpu_buffers
                .write_tile_metadata(&renderer.device, &renderer.queue, &tile_metadata)
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
        world.resources.insert(BackgroundBuffers {
            metadata_buffer: buffer,
            instances,
            tile_metadata_buffer,
            globe_tiles,
            atmosphere_metadata_buffer,
            sky_metadata_buffer,
        });
    }

    Ok(())
}

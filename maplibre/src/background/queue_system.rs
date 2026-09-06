use wgpu::util::DeviceExt;

use crate::{
    context::MapContext,
    render::{
        projection::globe_camera_for_view,
        render_phase::{DrawState, LayerItem, ProjectionBinding, RenderPhase, TranslucentItem},
        shaders::{AtmosphereLayerMetadata, BackgroundLayerMetadata, ShaderTileMetadata},
    },
    style::layer::LayerPaint,
    tcs::system::{SystemError, SystemResult},
};

/// GPU metadata shared by background and atmosphere draws.
pub struct BackgroundBuffers {
    /// Background layer paint metadata.
    pub metadata_buffer: wgpu::Buffer,
    /// Zoom-zero tile transform used by globe meshes.
    pub tile_metadata_buffer: wgpu::Buffer,
    /// Evaluated atmosphere opacity.
    pub atmosphere_metadata_buffer: wgpu::Buffer,
    /// Sky colours and horizon, when the style has a sky and the map is not a globe.
    pub sky_metadata_buffer: Option<wgpu::Buffer>,
}

use super::render_commands::{DrawAtmosphere, DrawBackground, DrawGlobeBackground, DrawSky};

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
    let projection_transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    let uses_globe = projection_transition > 0.0;
    let sky_blend = style.sky.as_ref().map_or(0.0, |sky| {
        sky.atmosphere_blend_at_zoom(view_state.zoom().value())
    });
    let atmosphere_blend = sky_blend * projection_transition;
    let sky_metadata = style
        .sky
        .as_ref()
        .filter(|_| projection_transition < 1.0)
        .map(|sky| sky_metadata(sky, view_state, projection_transition));

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
            background_index = background_index.max(layer.index);
            let c: [f32; 4] = match &layer.paint {
                Some(paint @ LayerPaint::Background(_)) => paint
                    .get_color()
                    .map(|c| c.into())
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]),
                _ => [0.0, 0.0, 0.0, 1.0],
            };
            let z_index = layer.index as f32;
            metadatas.push(BackgroundLayerMetadata {
                color: c,
                z_index,
                padding: [0.0; 3],
                horizon: view_state.horizon_line().to_shader(),
                viewport: [view_state.height() as f32, 0.0, 0.0, 0.0],
            });

            let draw_function: Box<dyn crate::render::render_phase::Draw<LayerItem>> = if uses_globe
            {
                Box::new(DrawState::<LayerItem, DrawGlobeBackground>::new())
            } else {
                Box::new(DrawState::<LayerItem, DrawBackground>::new())
            };
            layer_item_phase.add(LayerItem {
                projection: ProjectionBinding::View,
                draw_function,
                index: layer.index,
                is_line: false,
                generate_borders: false,
                style_layer: layer.id.clone(),
                source_shape: crate::render::tile_view_pattern::TileShape::default(),

                // We provide a dummy tile for background.
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
            });
        }
        // The sky follows the background layers and precedes everything else, as GL JS draws it
        // under the map; the flat map covers it below the horizon.
        if sky_metadata.is_some() {
            layer_item_phase.add(LayerItem {
                projection: ProjectionBinding::View,
                draw_function: Box::new(DrawState::<LayerItem, DrawSky>::new()),
                index: background_index,
                is_line: false,
                generate_borders: false,
                style_layer: "sky".to_string(),
                source_shape: crate::render::tile_view_pattern::TileShape::default(),
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
            });
        }
        if atmosphere_blend > 0.0 {
            translucent_phase.add(TranslucentItem {
                draw_function: Box::new(DrawState::<TranslucentItem, DrawAtmosphere>::new()),
                index: u32::MAX,
                style_layer: "atmosphere".to_string(),
                tile: crate::tcs::tiles::Tile {
                    coords: crate::coords::WorldTileCoords::default(),
                },
                source_shape: crate::render::tile_view_pattern::TileShape::default(),
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
                viewport: [view_state.height() as f32, 0.0, 0.0, 0.0],
            });
        }
        let buffer = renderer
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Background Metadata Buffer"),
                contents: bytemuck::cast_slice(&metadatas),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        let coords = crate::coords::WorldTileCoords::default();
        let transform = view_state
            .gpu_view_projection()
            .to_model_view_projection(coords.transform_for_zoom(view_state.zoom()))
            .downcast()
            .into();
        let tile_metadata = ShaderTileMetadata {
            transform,
            zoom_factor: view_state.zoom().scale_to_tile(&coords) as f32,
            viewport_width: view_state.width() as f32,
            viewport_height: view_state.height() as f32,
            tile_mercator_coords: crate::projection::renderer_data::tile_mercator_coordinates(
                Some(crate::coords::TileCoords::default()),
            )
            .into(),
            clip_antimeridian: 1,
        };
        let tile_metadata_buffer =
            renderer
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Globe Background Tile Metadata Buffer"),
                    contents: bytemuck::bytes_of(&tile_metadata),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
        let atmosphere_metadata = if atmosphere_blend > 0.0 {
            let camera = globe_camera_for_view(view_state).map_err(|error| {
                tracing::error!(?error, "failed to build atmosphere globe camera");
                SystemError::Setup
            })?;
            let light = style.light.clone().unwrap_or_default();
            AtmosphereLayerMetadata::from_view(
                &camera,
                &light,
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
        let atmosphere_metadata_buffer =
            renderer
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Atmosphere Metadata Buffer"),
                    contents: bytemuck::bytes_of(&atmosphere_metadata),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
        let sky_metadata_buffer = sky_metadata.map(|sky| {
            renderer
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Sky Metadata Buffer"),
                    contents: bytemuck::bytes_of(&sky),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                })
        });
        world.resources.insert(BackgroundBuffers {
            metadata_buffer: buffer,
            tile_metadata_buffer,
            atmosphere_metadata_buffer,
            sky_metadata_buffer,
        });
    }

    Ok(())
}

/// The sky draw's values for the frame: colours at the zoom and the horizon line on screen,
/// as GL JS `skyUniformValues` computes them.
fn sky_metadata(
    sky: &crate::style::sky::SkySpecification,
    view_state: &crate::render::view_state::ViewState,
    projection_transition: f32,
) -> crate::render::shaders::SkyLayerMetadata {
    let colors = sky.colors_at(view_state.zoom().value());
    let height = view_state.height();
    crate::render::shaders::SkyLayerMetadata {
        sky_color: colors.sky,
        horizon_color: colors.horizon,
        horizon: view_state.horizon_line().to_shader(),
        blend: [
            (f64::from(colors.sky_horizon_blend) * height / 2.0) as f32,
            projection_transition,
            height as f32,
            0.0,
        ],
    }
}

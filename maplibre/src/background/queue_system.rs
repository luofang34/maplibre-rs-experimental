//! Evaluates visible background paint and queues sky and atmosphere draws.

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
    /// Instance of each background style layer in `metadata_buffer`.
    pub instances: std::collections::HashMap<String, u32>,
    /// Zoom-zero tile transform used by globe meshes.
    pub tile_metadata_buffer: wgpu::Buffer,
    /// Evaluated atmosphere opacity.
    pub atmosphere_metadata_buffer: wgpu::Buffer,
    /// Sky colours and horizon, when the style has a sky and the map is not a globe.
    pub sky_metadata_buffer: Option<wgpu::Buffer>,
}

use super::render_commands::{
    DrawAtmosphere, DrawBackground, DrawBackgroundPattern, DrawGlobeBackground,
    DrawGlobeBackgroundPattern, DrawSky,
};

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
    let mut instances = std::collections::HashMap::new();
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
        .filter(|_| projection_transition < 1.0 || view_state.opaque_environment())
        .map(|sky| sky_metadata(sky, view_state, projection_transition));

    if let Some(gpu) = world
        .resources
        .get::<super::pattern::BackgroundPatternGpu>()
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

        let patterned: std::collections::HashSet<&str> = style
            .layers
            .iter()
            .filter(|layer| {
                layer.paint.as_ref().is_some_and(|paint| {
                    crate::vector::pattern::pattern_name(paint, view_state.style_zoom().value())
                        .is_some()
                })
            })
            .map(|layer| layer.id.as_str())
            .collect();
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
            background_index = background_index.max(layer.index);
            let c = background_color(layer, view_state.zoom().value());
            let z_index = layer.index as f32;
            instances.insert(layer.id.clone(), metadatas.len() as u32);
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

            let draw_function: Box<dyn crate::render::render_phase::Draw<LayerItem>> =
                if uses_globe && patterned.contains(layer.id.as_str()) {
                    Box::new(DrawState::<LayerItem, DrawGlobeBackgroundPattern>::new())
                } else if uses_globe {
                    Box::new(DrawState::<LayerItem, DrawGlobeBackground>::new())
                } else if patterned.contains(layer.id.as_str()) {
                    Box::new(DrawState::<LayerItem, DrawBackgroundPattern>::new())
                } else {
                    Box::new(DrawState::<LayerItem, DrawBackground>::new())
                };
            layer_item_phase.add(LayerItem {
                projection: ProjectionBinding::View,
                draw_function,
                index: layer.index,
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
                viewport: [
                    view_state.height() as f32,
                    f32::from(style.terrain.is_some()),
                    frame[0],
                    frame[1],
                ],
                world_rows,
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
            line_width_scale: 1.0,
            line_units_per_pixel: 8.0 * view_state.zoom().scale_to_tile(&coords) as f32,
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
            instances,
            tile_metadata_buffer,
            atmosphere_metadata_buffer,
            sky_metadata_buffer,
        });
    }

    Ok(())
}

/// The background colour at a zoom, with its opacity multiplied into premultiplied-free alpha.
fn background_color(layer: &crate::style::layer::StyleLayer, zoom: f64) -> [f32; 4] {
    let Some(LayerPaint::Background(paint)) = &layer.paint else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let mut color: [f32; 4] = paint
        .background_color
        .as_ref()
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .map_or([0.0, 0.0, 0.0, 1.0], |color| {
            cint::Alpha::<cint::EncodedSrgb<f32>>::from(color).into()
        });
    let opacity = paint
        .background_opacity
        .as_ref()
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .unwrap_or(1.0);
    color[3] *= opacity.clamp(0.0, 1.0);
    color
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
    // Zero depth denotes absent content to the host compositor. This distant positive
    // depth keeps sky coverage valid without visible parallax at the headset near plane.
    crate::render::shaders::SkyLayerMetadata {
        sky_color: colors.sky,
        horizon_color: colors.horizon,
        horizon: view_state.horizon_line().to_shader(),
        blend: [
            (f64::from(colors.sky_horizon_blend) * height / 2.0) as f32,
            if view_state.opaque_environment() {
                0.0
            } else {
                projection_transition
            },
            height as f32,
            if view_state.has_external_view() {
                1.0e-8
            } else {
                0.0
            },
        ],
    }
}

/// The rows that tell a pixel's map row for the northern and southern edges of the flat map,
/// or ones that cut nothing where the map has no such edges.
fn world_rows(view_state: &crate::render::view_state::ViewState, edges: bool) -> [[f32; 4]; 2] {
    use cgmath::{Matrix3, Matrix4, SquareMatrix, Vector3};

    const OPEN: [[f32; 4]; 2] = [[0.0, 0.0, 0.0, -1e30], [0.0, 0.0, 0.0, 1e30]];
    if !edges {
        return OPEN;
    }
    let center = view_state.camera().position();
    let relative = view_state.view_projection().0
        * Matrix4::from_translation(Vector3::new(center.x, center.y, 0.0));
    // A point of the map plane is (x, y, 0, 1); the clip x, y and w it produces come from
    // the matrix columns of x, y and the constant.
    let plane = Matrix3::new(
        relative.x.x,
        relative.x.y,
        relative.x.w,
        relative.y.x,
        relative.y.y,
        relative.y.w,
        relative.w.x,
        relative.w.y,
        relative.w.w,
    );
    let Some(inverse) = plane.invert() else {
        return OPEN;
    };
    let world = crate::coords::TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    // Rows of the inverse give the map x, y and the scale a pixel's clip position carries.
    [
        [
            inverse.x.y as f32,
            inverse.y.y as f32,
            inverse.z.y as f32,
            (-center.y) as f32,
        ],
        [
            inverse.x.z as f32,
            inverse.y.z as f32,
            inverse.z.z as f32,
            (world - center.y) as f32,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::background_color;
    use crate::style::layer::StyleLayer;

    fn layer(paint: serde_json::Value) -> StyleLayer {
        let layer = serde_json::json!({"id": "b", "type": "background", "paint": paint});
        serde_json::from_value(layer).expect("valid background layer")
    }

    #[test]
    fn opacity_scales_alpha() {
        let color = background_color(
            &layer(serde_json::json!({"background-color": "blue", "background-opacity": 0.5})),
            0.0,
        );
        assert_eq!(color, [0.0, 0.0, 1.0, 0.5]);
    }

    #[test]
    fn colour_follows_zoom() {
        let paint = serde_json::json!({"background-color": {
            "stops": [[0, "red"], [10, "blue"]], "base": 1.0
        }});
        let start = background_color(&layer(paint.clone()), 0.0);
        let end = background_color(&layer(paint), 10.0);
        assert_eq!(start[..3], [1.0, 0.0, 0.0]);
        assert_eq!(end[..3], [0.0, 0.0, 1.0]);
    }
}

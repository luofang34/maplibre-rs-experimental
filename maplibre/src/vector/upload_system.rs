//! Uploads data to the GPU which is needed for rendering.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::UPLOADS_PER_FRAME,
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata, Vec4f32},
        tile_view_pattern::WgpuTileViewPattern,
        Renderer,
    },
    style::{
        circle::{CirclePitchAlignment, CirclePitchScale},
        layer::{LayerPaint, TranslateAnchor},
        Style,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tiles,
    },
    vector::{
        AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

pub(crate) mod paint;

pub(crate) fn drape_paint_zoom(zoom: f64) -> f64 {
    (zoom * 8.0).round() / 8.0
}

#[derive(Clone, Copy, PartialEq)]
struct VectorPaintFrame {
    zoom: f32,
    bearing: f32,
}

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    let (paint_frame, previous_paint) = frame_paint(world, style, view_state);
    let VectorPaintFrame { zoom, bearing } = paint_frame;
    let refresh_paint = previous_paint != Some(paint_frame);
    let Some(Initialized(pattern)) = world.resources.get::<Eventually<WgpuTileViewPattern>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let mut source_tiles = if style.terrain.is_some() {
        vec![crate::coords::WorldTileCoords::default()]
    } else {
        sources_for_upload(pattern)
    };
    if let Some(requests) = world
        .resources
        .get::<crate::terrain::request_system::DrapeRequests>()
    {
        let mut seen: HashSet<_> = source_tiles.iter().copied().collect();
        source_tiles.extend(requests.0.iter().copied().filter(|tile| seen.insert(*tile)));
    }
    let painted_tiles = painted_tiles(pattern, &source_tiles);
    let (spatial, changed) = super::structures::for_frame(world, style, &painted_tiles);
    let Some(Initialized(buffer_pool)) = world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        return Err(SystemError::Dependencies);
    };
    if refresh_paint {
        refresh_layer_paint(
            buffer_pool,
            queue,
            &painted_tiles,
            zoom,
            bearing,
            previous_paint,
        );
    }
    if changed {
        super::structures::refresh_gpu(buffer_pool, queue, &spatial);
    }
    upload_tessellated_layer(
        buffer_pool,
        queue,
        &mut world.tiles,
        style,
        source_tiles,
        &spatial,
        zoom,
        bearing,
    );
    Ok(())
}

fn frame_paint(
    world: &mut crate::tcs::world::World,
    style: &Style,
    view_state: &crate::render::view_state::ViewState,
) -> (VectorPaintFrame, Option<VectorPaintFrame>) {
    let zoom = if style.terrain.is_some() {
        paint::stabilize_zoom(world, view_state.style_zoom().value()) as f32
    } else {
        view_state.style_zoom().level()
    };
    let bearing = view_state.camera().get_bearing().0 as f32;
    let paint_frame = VectorPaintFrame { zoom, bearing };
    let previous_paint = world.resources.get::<VectorPaintFrame>().copied();
    world.resources.insert(paint_frame);
    (paint_frame, previous_paint)
}

fn painted_tiles(
    pattern: &WgpuTileViewPattern,
    source_tiles: &[crate::coords::WorldTileCoords],
) -> Vec<crate::coords::WorldTileCoords> {
    let mut painted_tiles = source_tiles.to_vec();
    let mut seen: HashSet<_> = painted_tiles.iter().copied().collect();
    for tile in pattern.iter() {
        tile.render_kind(TileKind::Vector, |shape| {
            if seen.insert(shape.coords()) {
                painted_tiles.push(shape.coords());
            }
        });
    }
    painted_tiles
}

fn sources_for_upload(
    tile_view_pattern: &WgpuTileViewPattern,
) -> Vec<crate::coords::WorldTileCoords> {
    let mut source_tiles = Vec::new();
    let mut seen = HashSet::new();
    for view_tile in tile_view_pattern.iter() {
        // The view tile itself is uploaded whether or not the frame draws it yet: a tile
        // counts as available only once it is in the pool, so the frame draws a stand-in
        // until this upload has happened.
        if seen.insert(view_tile.coords()) {
            source_tiles.push(view_tile.coords());
        }
        view_tile.render_kind(TileKind::Vector, |shape| {
            if seen.insert(shape.coords()) {
                source_tiles.push(shape.coords());
            }
        });
    }
    source_tiles
}

fn refresh_layer_paint(
    buffer_pool: &VectorBufferPool,
    queue: &wgpu::Queue,
    source_tiles: &[crate::coords::WorldTileCoords],
    zoom: f32,
    bearing: f32,
    previous: Option<VectorPaintFrame>,
) {
    for coords in source_tiles {
        for entry in buffer_pool
            .index()
            .get_layers(*coords)
            .into_iter()
            .flatten()
        {
            let metadata = metadata_for_layer(&entry.style_layer, *coords, zoom, bearing);
            let unchanged = previous.is_some_and(|frame| {
                bytemuck::bytes_of(&metadata_for_layer(
                    &entry.style_layer,
                    *coords,
                    frame.zoom,
                    frame.bearing,
                )) == bytemuck::bytes_of(&metadata)
            });
            if !unchanged {
                buffer_pool.update_layer_metadata(queue, entry, metadata);
            }
            if let Some(color) = paint::uniform_color(&entry.style_layer, f64::from(zoom)) {
                if previous
                    .map(|frame| frame.zoom)
                    .and_then(|zoom| paint::uniform_color(&entry.style_layer, f64::from(zoom)))
                    == Some(color)
                {
                    continue;
                }
                let count = (entry.feature_metadata_buffer_range().end
                    - entry.feature_metadata_buffer_range().start)
                    as usize
                    / std::mem::size_of::<FillShaderFeatureMetadata>();
                let colors = vec![FillShaderFeatureMetadata { color }; count];
                buffer_pool.update_feature_metadata(queue, entry, &colors);
            }
        }
    }
}

fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &wgpu::Queue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: Vec<crate::coords::WorldTileCoords>,
    spatial: &[super::structures::SpatialBuffer],
    zoom: f32,
    bearing: f32,
) {
    // Upload the tessellated layers in view, a few tiles a frame; the rest follow next
    // frame rather than staging a whole burst of arrivals at once.
    let mut uploaded_tiles = 0;
    let mut bytes = crate::render::memory_budget::UploadBudget::new(16 << 20);
    for coords in source_tiles {
        if uploaded_tiles >= UPLOADS_PER_FRAME {
            break;
        }
        let Some(vector_layers) = tiles.query_mut::<&VectorLayerBucketComponent>(coords) else {
            continue;
        };

        let loaded_layers = buffer_pool
            .get_loaded_style_layers_at(coords)
            .unwrap_or_default();

        let available_layers = vector_layers
            .layers
            .iter()
            .flat_map(|data| match data {
                VectorLayerBucket::AvailableLayer(data) => Some(data),
                VectorLayerBucket::Missing(_) => None,
            })
            .filter(|data| !loaded_layers.contains(data.style_layer_id.as_str()))
            // Empty buckets never enter the pool and must not consume a slot every frame.
            .filter(|data| !data.buffer.buffer.indices.is_empty())
            .collect::<Vec<_>>();

        for style_layer in &style.layers {
            let Some(AvailableVectorLayerBucket {
                coords,
                feature_indices,
                feature_colors,
                buffer,
                ..
            }) = available_layers
                .iter()
                .find(|layer| style_layer.id.as_str() == layer.style_layer_id.as_str())
            else {
                continue;
            };

            let size = buffer.buffer.vertices.len() * size_of::<crate::render::ShaderVertex>()
                + buffer.buffer.indices.len() * size_of::<u32>()
                + feature_indices
                    .iter()
                    .map(|count| *count as usize)
                    .sum::<usize>()
                    * size_of::<FillShaderFeatureMetadata>();
            if !bytes.take(size) {
                return;
            }
            upload_bucket(
                buffer_pool,
                queue,
                style_layer,
                (*coords, buffer, feature_indices, feature_colors),
                spatial,
                zoom,
                bearing,
            );
        }
        if !available_layers.is_empty() {
            uploaded_tiles += 1;
        }
    }
}

fn upload_bucket(
    buffer_pool: &mut VectorBufferPool,
    queue: &wgpu::Queue,
    style_layer: &crate::style::layer::StyleLayer,
    data: (
        crate::coords::WorldTileCoords,
        &crate::vector::tessellation::OverAlignedVertexBuffer<crate::render::ShaderVertex, u32>,
        &[u32],
        &[[f32; 4]],
    ),
    spatial: &[super::structures::SpatialBuffer],
    zoom: f32,
    bearing: f32,
) {
    let (coords, buffer, feature_indices, feature_colors) = data;
    let color: Option<Vec4f32> = style_layer
        .paint
        .as_ref()
        .and_then(|paint| paint.get_color())
        .map(|color| color.into());

    // Assign every feature in the layer the color from the style if no parsed feature_color exist.
    let fallback_color = color.unwrap_or([0.0, 0.0, 0.0, 1.0]);

    let feature_metadata = if let Some(color) = paint::uniform_color(style_layer, f64::from(zoom)) {
        feature_metadata(feature_indices, &[], color)
    } else {
        feature_metadata(feature_indices, feature_colors, fallback_color)
    };

    let buffer = spatial
        .iter()
        .find(|(tile, id, _)| *tile == coords && *id == style_layer.id)
        .map_or(buffer, |(_, _, spatial)| spatial);
    let layer_metadata = metadata_for_layer(style_layer, coords, zoom, bearing);

    tracing::debug!(%coords, "allocating vector geometry");
    if let Err(error) = buffer_pool.allocate_layer_geometry(
        queue,
        coords,
        style_layer.clone(),
        buffer,
        layer_metadata,
        &feature_metadata,
    ) {
        tracing::error!(%coords, %error, "tile geometry upload failed");
    }
}

fn layer_translate_tile_units(
    paint: Option<&LayerPaint>,
    tile_zoom: crate::coords::ZoomLevel,
    view_zoom: f32,
    bearing: f32,
) -> [f32; 2] {
    let (translate, anchor) = match paint {
        Some(LayerPaint::Fill(paint)) => (
            paint.fill_translate.unwrap_or([0.0; 2]),
            paint.fill_translate_anchor,
        ),
        Some(LayerPaint::Line(paint)) => (
            paint.line_translate.unwrap_or([0.0; 2]),
            paint.line_translate_anchor,
        ),
        Some(LayerPaint::Circle(paint)) => (
            paint.circle_translate.unwrap_or([0.0; 2]),
            paint.circle_translate_anchor,
        ),
        _ => return [0.0; 2],
    };
    let translated = if anchor == TranslateAnchor::Viewport {
        let (sin, cos) = bearing.sin_cos();
        [
            translate[0] * cos - translate[1] * sin,
            translate[0] * sin + translate[1] * cos,
        ]
    } else {
        translate
    };
    let pixels_to_tile_units = 8.0 * 2.0_f32.powf(f32::from(u8::from(tile_zoom)) - view_zoom);
    [
        translated[0] * pixels_to_tile_units,
        translated[1] * pixels_to_tile_units,
    ]
}

#[cfg(test)]
mod tests;

fn metadata_for_layer(
    style_layer: &crate::style::layer::StyleLayer,
    coords: crate::coords::WorldTileCoords,
    zoom: f32,
    bearing: f32,
) -> ShaderLayerMetadata {
    // Extract line-width from style paint (default 1.0px)
    let line_width = match &style_layer.paint {
        Some(LayerPaint::Line(paint)) => paint
            .line_width
            .as_ref()
            .and_then(|w| w.evaluate_at_zoom(f64::from(zoom)))
            .unwrap_or(1.0),
        _ => 1.0,
    };
    let translate = layer_translate_tile_units(style_layer.paint.as_ref(), coords.z, zoom, bearing);
    let mut layer_metadata =
        ShaderLayerMetadata::new(style_layer.index as f32, line_width, translate);
    if let Some(LayerPaint::Circle(paint)) = &style_layer.paint {
        let zoom = f64::from(zoom);
        layer_metadata.stroke_color = paint.stroke_color_rgba();
        // Fill opacity is already folded into each feature's colour alpha.
        layer_metadata.circle_params =
            [1.0, paint.stroke_opacity_at(zoom), paint.blur_at(zoom), 0.0];
        layer_metadata.circle_flags = [
            f32::from(paint.circle_pitch_scale == CirclePitchScale::Map),
            f32::from(paint.circle_pitch_alignment == CirclePitchAlignment::Map),
            0.0,
            0.0,
        ];
    }

    layer_metadata
}

fn feature_metadata(
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
) -> Vec<FillShaderFeatureMetadata> {
    let mut feature_metadata = Vec::with_capacity(feature_indices.iter().sum::<u32>() as usize);
    for (idx, &count) in feature_indices.iter().enumerate() {
        let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
        for _ in 0..count {
            feature_metadata.push(FillShaderFeatureMetadata {
                color: current_color,
            });
        }
    }

    feature_metadata
}

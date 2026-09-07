//! Uploads data to the GPU which is needed for rendering.

use std::collections::BTreeSet;

use crate::{
    context::MapContext,
    io::tile_sources::TileKind,
    render::{
        eventually::{Eventually, Eventually::Initialized},
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

pub fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((Initialized(buffer_pool), Initialized(tile_view_pattern))) =
        world.resources.query_mut::<(
            &mut Eventually<VectorBufferPool>,
            &Eventually<WgpuTileViewPattern>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    let zoom = view_state.zoom().level();
    let bearing = view_state.camera().get_bearing().0 as f32;
    let mut source_tiles = BTreeSet::new();
    for view_tile in tile_view_pattern.iter() {
        // The view tile itself is uploaded whether or not the frame draws it yet: a tile
        // counts as available only once it is in the pool, so the frame draws a stand-in
        // until this upload has happened.
        source_tiles.insert(view_tile.coords());
        view_tile.render_kind(TileKind::Vector, |shape| {
            source_tiles.insert(shape.coords());
        });
    }
    upload_tessellated_layer(
        buffer_pool,
        queue,
        &mut world.tiles,
        style,
        source_tiles,
        zoom,
        bearing,
    );

    Ok(())
}

fn upload_tessellated_layer(
    buffer_pool: &mut VectorBufferPool,
    queue: &wgpu::Queue,
    tiles: &mut Tiles,
    style: &Style,
    source_tiles: BTreeSet<crate::coords::WorldTileCoords>,
    zoom: f32,
    bearing: f32,
) {
    // Upload all tessellated layers which are in view
    for coords in source_tiles {
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

            let color: Option<Vec4f32> = style_layer
                .paint
                .as_ref()
                .and_then(|paint| paint.get_color())
                .map(|color| color.into());

            // Assign every feature in the layer the color from the style if no parsed feature_color exist.
            let fallback_color = color.unwrap_or([0.0, 0.0, 0.0, 1.0]);

            let mut feature_metadata =
                Vec::with_capacity(feature_indices.iter().sum::<u32>() as usize);
            for (idx, &count) in feature_indices.iter().enumerate() {
                let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
                for _ in 0..count {
                    feature_metadata.push(FillShaderFeatureMetadata {
                        color: current_color,
                    });
                }
            }

            // FIXME avoid uploading empty indices
            if buffer.buffer.indices.is_empty() {
                continue;
            }

            // Extract line-width from style paint (default 1.0px)
            let line_width = match &style_layer.paint {
                Some(LayerPaint::Line(paint)) => paint
                    .line_width
                    .as_ref()
                    .and_then(|w| w.evaluate_at_zoom(f64::from(zoom)))
                    .unwrap_or(1.0),
                _ => 1.0,
            };
            let translate =
                layer_translate_tile_units(style_layer.paint.as_ref(), coords.z, zoom, bearing);
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

            log::debug!("Allocating geometry at {coords}");
            buffer_pool.allocate_layer_geometry(
                queue,
                *coords,
                style_layer.clone(),
                buffer,
                layer_metadata,
                &feature_metadata,
            );
        }
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

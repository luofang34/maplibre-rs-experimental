//! Paint shared by every feature must use the view zoom, including overzoomed source tiles.
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    metadata::{
        metadata_for_layer, paint_depends_on_bearing, paint_metadata_depends_on_zoom,
        same_quantized_rgba, uniform_color_depends_on_zoom, uses_layer_metadata_uniform_color,
    },
    VectorPaintFrame,
};
use crate::{
    render::shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
    style::layer::{LayerPaint, StyleLayer, StyleProperty},
    vector::VectorBufferPool,
};

pub(super) fn refresh_layer_paint(
    buffer_pool: &VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    source_tiles: &[crate::coords::WorldTileCoords],
    frame: VectorPaintFrame,
    previous: Option<VectorPaintFrame>,
) {
    let VectorPaintFrame { zoom, .. } = frame;
    let zoom_changed = previous.is_none_or(|prev| prev.zoom != zoom);
    let bearing_changed = previous.is_none_or(|prev| prev.bearing != frame.bearing);
    let light_changed = previous.is_none_or(|prev| prev.light != frame.light);
    let mut meta_memo: FxHashMap<(u32, u8), (ShaderLayerMetadata, bool)> = FxHashMap::default();
    let mut updated_layer_slots: FxHashSet<(u16, u64)> = FxHashSet::default();
    let mut color_memo: FxHashMap<u32, Option<[f32; 4]>> = FxHashMap::default();

    for coords in source_tiles {
        let tile_z = u8::from(coords.z);
        for entry in buffer_pool
            .index()
            .get_layers(*coords)
            .into_iter()
            .flatten()
        {
            let paint = entry.style_layer.paint.as_ref();
            let meta_dirty = (zoom_changed && paint_metadata_depends_on_zoom(paint))
                || (bearing_changed && paint_depends_on_bearing(paint))
                || (light_changed && matches!(paint, Some(LayerPaint::FillExtrusion(_))));
            if meta_dirty {
                let (metadata, changed) = *meta_memo
                    .entry((entry.style_layer.index, tile_z))
                    .or_insert_with(|| {
                        let metadata = metadata_for_layer(&entry.style_layer, *coords, frame);
                        let changed = previous.is_none_or(|prev| {
                            bytemuck::bytes_of(&metadata_for_layer(
                                &entry.style_layer,
                                *coords,
                                prev,
                            )) != bytemuck::bytes_of(&metadata)
                        });
                        (metadata, changed)
                    });
                if changed
                    && updated_layer_slots
                        .insert((entry.page(), entry.layer_metadata_buffer_range().start))
                {
                    buffer_pool.update_layer_metadata(queue, entry, metadata);
                }
            }
            // Uniform fill and line colours already ride in `ShaderLayerMetadata::stroke_color`.
            if !zoom_changed
                || uses_layer_metadata_uniform_color(&entry.style_layer)
                || !uniform_color_depends_on_zoom(&entry.style_layer)
            {
                continue;
            }
            let changed_color = *color_memo
                .entry(entry.style_layer.index)
                .or_insert_with(|| {
                    let color = uniform_color(&entry.style_layer, f64::from(zoom))?;
                    let unchanged = previous
                        .and_then(|prev| uniform_color(&entry.style_layer, f64::from(prev.zoom)))
                        .is_some_and(|prev_color| same_quantized_rgba(prev_color, color));
                    (!unchanged).then_some(color)
                });
            if let Some(color) = changed_color {
                buffer_pool.fill_feature_metadata(
                    queue,
                    entry,
                    FillShaderFeatureMetadata { color },
                );
            }
        }
    }
}

/// White with the opacity of a fill that repeats an image, unless the opacity varies by feature.
fn pattern_color(opacity: Option<&StyleProperty<f32>>, zoom: f64) -> Option<[f32; 4]> {
    if opacity.is_some_and(|opacity| !opacity.is_feature_constant()) {
        return None;
    }
    let opacity = opacity
        .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([1.0, 1.0, 1.0, opacity])
}

pub(crate) fn uniform_color(layer: &StyleLayer, zoom: f64) -> Option<[f32; 4]> {
    let (color, opacity) = match layer.paint.as_ref()? {
        // A pattern takes only the opacity of the fill.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            // A pattern that varies by feature rides in the feature colours.
            if super::super::pattern::per_feature_pattern(layer.paint.as_ref()?).is_some() {
                return None;
            }
            return pattern_color(paint.fill_opacity.as_ref(), zoom);
        }
        // The outline is a feature of its own, with its own colour.
        LayerPaint::Fill(paint) if paint.fill_outline_color.is_some() => return None,
        LayerPaint::Fill(paint) => (&paint.fill_color, &paint.fill_opacity),
        // A gradient's colour comes from its ramp, so the feature colours carry the length.
        LayerPaint::Line(paint) if paint.line_gradient.is_some() => return None,
        // A pattern that varies by feature rides in the feature colours.
        LayerPaint::Line(paint) if paint.line_pattern.is_some() => {
            if super::super::pattern::per_feature_pattern(layer.paint.as_ref()?).is_some() {
                return None;
            }
            (&paint.line_color, &paint.line_opacity)
        }
        LayerPaint::Line(paint) => (&paint.line_color, &paint.line_opacity),
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            if super::super::pattern::per_feature_pattern(layer.paint.as_ref()?).is_some() {
                return None;
            }
            return if paint.fill_extrusion_translucent == Some(true) {
                pattern_color(paint.fill_extrusion_opacity.as_ref(), zoom)
            } else {
                Some([1.0, 1.0, 1.0, 1.0])
            };
        }
        LayerPaint::FillExtrusion(paint) => {
            let opacity = if paint.fill_extrusion_translucent == Some(true) {
                &paint.fill_extrusion_opacity
            } else {
                &None
            };
            (&paint.fill_extrusion_color, opacity)
        }
        _ => return None,
    };
    if color.as_ref().is_some_and(|v| !v.is_feature_constant())
        || opacity.as_ref().is_some_and(|v| !v.is_feature_constant())
    {
        return None;
    }
    let color = color
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or_default();
    let opacity = opacity
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([color.r, color.g, color.b, color.a * opacity])
}

#[cfg(test)]
mod tests;

#[derive(Default)]
struct PaintZoom(Option<f64>);

pub(super) fn stabilize_zoom(world: &mut crate::tcs::world::World, requested: f64) -> f64 {
    let state = world.resources.get_or_init_mut::<PaintZoom>();
    let zoom = next_zoom(state.0, requested);
    state.0 = Some(zoom);
    zoom
}

fn next_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|previous| (requested - previous).abs() < 0.125)
        .unwrap_or_else(|| super::drape_paint_zoom(requested))
}

pub(crate) fn current_zoom(world: &crate::tcs::world::World, fallback: f64) -> f64 {
    world
        .resources
        .get::<PaintZoom>()
        .and_then(|state| state.0)
        .unwrap_or_else(|| super::drape_paint_zoom(fallback))
}

//! Applies one style entry to an isolated copy of its source layer.
use super::*;

pub(super) fn process_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    if crate::geojson::reads_geometry(style) {
        super::geometry_tags::add_geometry_tags(&mut layer, request.coords);
    }
    if crate::geojson::reads_identity(style) {
        super::geometry_tags::add_identity_tags(&mut layer);
    }
    if matches!(style.paint, Some(LayerPaint::FillExtrusion(_))) {
        unhide_unreplaced_building_bases(&mut layer);
    }
    if let Some(filter) = &style.filter {
        match Filter::parse(filter) {
            Ok(filter) => apply_filter_to_layer(&mut layer, &filter, request.style_zoom()),
            Err(error) => {
                // Rendering either all features or none silently would conceal an invalid style.
                tracing::error!(layer = %style.id,%error,"unsupported filter; the layer renders nothing");
                context.layer_missing(&request.coords, &layer.name)?;
                return Ok(());
            }
        }
    }
    match style.paint.as_ref() {
        Some(
            paint @ (LayerPaint::Line(_)
            | LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_)),
        ) => vector_layer(layer, style, paint, request, context),
        Some(LayerPaint::Symbol(paint)) => {
            symbol_layer(layer, style, paint, request, context, atlas)
        }
        _ => {
            tracing::warn!(layer = %style.id,"unhandled vector style layer type");
            Ok(())
        }
    }
}

fn tessellator(paint: &LayerPaint, request: &VectorTileRequest) -> ZeroTessellator<IndexDataType> {
    let zoom = u8::from(request.coords.z);
    let style_zoom = request.style_zoom();
    let granularity = match paint {
        LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
        LayerPaint::Line(_) => granularity_for_zoom(512, 0, zoom),
        _ => 1,
    };
    let mut tessellator = match paint {
        LayerPaint::Circle(circle) => ZeroTessellator::default().with_circles(CircleOptions {
            // A circle lying on the map follows the globe's curve.
            grid: request.projection.uses_globe_rendering(style_zoom)
                && circle.circle_pitch_alignment == crate::style::circle::CirclePitchAlignment::Map,
            ..CircleOptions::for_paint(circle, style_zoom)
        }),
        LayerPaint::Heatmap(heatmap) => ZeroTessellator::default().with_circles(CircleOptions {
            grid: request.projection.uses_globe_rendering(style_zoom),
            ..CircleOptions::for_heatmap(heatmap, style_zoom)
        }),
        LayerPaint::FillExtrusion(extrusion) => {
            let tessellator = ZeroTessellator::default().with_extrusion(
                ExtrusionOptions::for_paint(extrusion, request.overscaling()),
            );
            if request.projection.uses_globe_rendering(style_zoom) {
                // Roofs and the edges of walls follow the sphere as fills and lines do.
                let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
                tessellator.with_globe_subdivision(
                    granularity_for_zoom(128, 2, zoom),
                    zoom == 0,
                    request.coords.y == 0,
                    i64::from(request.coords.y) == last_tile,
                )
            } else {
                tessellator
            }
        }
        _ if request.projection.uses_globe_rendering(style_zoom) => {
            let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
            ZeroTessellator::default().with_globe_subdivision(
                granularity,
                zoom == 0,
                request.coords.y == 0,
                i64::from(request.coords.y) == last_tile,
            )
        }
        _ => ZeroTessellator::default(),
    }
    .with_feature_opacity(paint.opacity(), style_zoom);
    match paint {
        // An image repeated over a fill takes nothing from the fill's colour but its opacity.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property =
                crate::vector::pattern::per_feature_pattern(&LayerPaint::Fill(paint.clone()));
        }
        LayerPaint::Fill(paint) => {
            tessellator.style_property = paint.fill_color.clone();
            tessellator.outline_property = paint.outline_color();
        }
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property = crate::vector::pattern::per_feature_pattern(
                &LayerPaint::FillExtrusion(paint.clone()),
            );
        }
        LayerPaint::FillExtrusion(paint) => {
            tessellator.style_property = paint.fill_extrusion_color.clone()
        }
        LayerPaint::Circle(paint) => tessellator.style_property = paint.circle_color.clone(),
        LayerPaint::Line(paint) => {
            tessellator.pattern_property =
                crate::vector::pattern::per_feature_pattern(&LayerPaint::Line(paint.clone()));
            tessellator.style_property = paint.line_color.clone();
            tessellator.is_line_layer = true;
            tessellator.line_gradient = paint.line_gradient.is_some();
            tessellator.sharp_corner_offset =
                crate::vector::tessellation::sharp_corner_offset(request.overscaling());
            tessellator.split_corners = paint.line_gradient.is_some()
                || paint.line_pattern.is_some()
                || paint.line_dasharray.is_some();
            tessellator.line_feature_style =
                super::super::tessellation::LineFeatureStyle::for_paint(paint, style_zoom);
        }
        _ => {}
    }
    tessellator
}

fn vector_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    paint: &LayerPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let build = |layer: &mut tile::Layer| {
        let mut tessellator = tessellator(paint, request);
        tessellator.set_layer_strokes(style);
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.sort_key = crate::vector::tessellation::SortKeys::by(
            crate::vector::tessellation::sort_key_of(style),
        );
        layer.process(&mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(&mut layer, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.apply_sort_keys();
    let sort_keys = tessellator.sort_key_values();
    context.layer_tessellation_finished(
        &request.coords,
        tessellator.buffer.into(),
        tessellator.feature_indices,
        (tessellator.feature_colors, sort_keys),
        layer,
        style.id.clone(),
    )
}

/// Tessellates the layer with `build`. When a feature's geometry cannot be read, the features
/// whose geometry cannot are left out and the rest are tessellated, so one malformed feature
/// does not take the layer with it; `None` when the layer cannot be tessellated even so.
fn tessellate_readable<Tessellator>(
    layer: &mut tile::Layer,
    coords: WorldTileCoords,
    mut build: impl FnMut(&mut tile::Layer) -> geozero::error::Result<Tessellator>,
) -> Option<Tessellator> {
    use geozero::{GeozeroGeometry, ProcessorSink};
    let error = match build(layer) {
        Ok(tessellator) => return Some(tessellator),
        Err(error) => error,
    };
    let before = layer.features.len();
    layer
        .features
        .retain(|feature| feature.process_geom(&mut ProcessorSink).is_ok());
    let dropped = before - layer.features.len();
    if dropped == 0 {
        tracing::error!(%coords, layer = %layer.name, ?error, "vector tessellation failed");
        return None;
    }
    tracing::warn!(%coords, layer = %layer.name, dropped, ?error, "features whose geometry cannot be read are left out");
    match build(layer) {
        Ok(tessellator) => Some(tessellator),
        Err(error) => {
            tracing::error!(%coords, layer = %layer.name, ?error, "vector tessellation failed");
            None
        }
    }
}

fn symbol_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    paint: &crate::style::layer::SymbolPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    let zoom = request.style_zoom();
    let build = |layer: &mut tile::Layer| {
        let mut tessellator = TextTessellator::with_assets(paint.clone(), zoom, atlas.clone());
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.overscaling = request.overscaling();
        tessellator.source_ids = layer.features.iter().map(|feature| feature.id).collect();
        layer.process(&mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(&mut layer, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.finish();
    let has_geometry = !tessellator.quad_buffer.indices.is_empty();
    context.symbol_layer_tessellation_finished(
        crate::vector::transferables::DefaultSymbolLayerTessellated {
            coords: request.coords,
            buffer: tessellator.quad_buffer.into(),
            features: tessellator.features,
            atlas: has_geometry.then_some(atlas),
            layer_data: layer,
            style_layer_id: style.id.clone(),
        },
    )
}

/// Clears `hide_3d` on building footprints whose interior `building:part` pieces all start
/// above ground (`render_min_height > 0`), so stepped skyscrapers modeled with a `building`
/// base podium and elevated `building:part` tiers do not float in mid-air.
fn unhide_unreplaced_building_bases(layer: &mut tile::Layer) {
    let Some(hide_3d_key) = layer
        .keys
        .iter()
        .position(|key| key == "hide_3d")
        .and_then(|idx| u32::try_from(idx).ok())
    else {
        return;
    };
    let Some(min_height_key) = layer
        .keys
        .iter()
        .position(|key| key == "render_min_height")
        .and_then(|idx| u32::try_from(idx).ok())
    else {
        return;
    };
    let height_key = layer
        .keys
        .iter()
        .position(|key| key == "render_height")
        .and_then(|idx| u32::try_from(idx).ok());

    struct PartInfo {
        bbox: [i32; 4],
        centroids: Vec<[f32; 2]>,
        min_height: f64,
    }

    let mut parts = Vec::new();
    let mut has_hide_3d = false;
    for feature in &layer.features {
        if feature.r#type != Some(3) {
            continue;
        }
        if feature_bool_tag(feature, hide_3d_key, &layer.values) == Some(true) {
            has_hide_3d = true;
            continue;
        }
        let rings = decode_mvt_rings(&feature.geometry);
        let Some(bbox) = rings_bbox(&rings) else {
            continue;
        };
        let centroids: Vec<[f32; 2]> = rings
            .iter()
            .filter_map(|ring| {
                if ring.is_empty() {
                    return None;
                }
                let n = ring.len() as f32;
                let sum = ring.iter().fold([0.0_f32, 0.0_f32], |acc, pt| {
                    [acc[0] + pt[0] as f32, acc[1] + pt[1] as f32]
                });
                Some([sum[0] / n, sum[1] / n])
            })
            .collect();
        let min_height = feature_number_tag(feature, min_height_key, &layer.values).unwrap_or(0.0);
        parts.push(PartInfo {
            bbox,
            centroids,
            min_height,
        });
    }
    if !has_hide_3d || parts.is_empty() {
        return;
    }

    for feature_index in 0..layer.features.len() {
        let feature = &layer.features[feature_index];
        if feature.r#type != Some(3)
            || feature_bool_tag(feature, hide_3d_key, &layer.values) != Some(true)
            || feature_number_tag(feature, min_height_key, &layer.values).unwrap_or(0.0) > 0.0
        {
            continue;
        }
        let rings = decode_mvt_rings(&feature.geometry);
        let Some(bbox) = rings_bbox(&rings) else {
            continue;
        };
        let mut min_part_base: Option<f64> = None;
        for part in &parts {
            if part.bbox[2] < bbox[0]
                || part.bbox[0] > bbox[2]
                || part.bbox[3] < bbox[1]
                || part.bbox[1] > bbox[3]
            {
                continue;
            }
            let inside = part.centroids.iter().any(|&point| {
                rings
                    .iter()
                    .filter(|ring| point_in_mvt_ring(ring, point))
                    .count()
                    % 2
                    == 1
            });
            if inside {
                min_part_base =
                    Some(min_part_base.map_or(part.min_height, |cur| cur.min(part.min_height)));
            }
        }
        let Some(min_base) = min_part_base.filter(|&base| base > 0.0) else {
            continue;
        };
        remove_feature_tag(&mut layer.features[feature_index].tags, hide_3d_key);
        if let Some(height_key) = height_key {
            if let Some(height) =
                feature_number_tag(&layer.features[feature_index], height_key, &layer.values)
            {
                if height > min_base {
                    let val_idx = u32::try_from(layer.values.len()).unwrap_or(u32::MAX);
                    layer.values.push(tile::Value {
                        double_value: Some(min_base),
                        ..Default::default()
                    });
                    set_feature_tag(&mut layer.features[feature_index].tags, height_key, val_idx);
                }
            }
        }
    }
}

fn feature_bool_tag(feature: &tile::Feature, key: u32, values: &[tile::Value]) -> Option<bool> {
    for &[k, v] in feature.tags.as_chunks::<2>().0 {
        if k == key {
            return values.get(v as usize)?.bool_value;
        }
    }
    None
}

fn feature_number_tag(feature: &tile::Feature, key: u32, values: &[tile::Value]) -> Option<f64> {
    for &[k, v] in feature.tags.as_chunks::<2>().0 {
        if k == key {
            let v = values.get(v as usize)?;
            return v
                .double_value
                .or_else(|| v.float_value.map(f64::from))
                .or_else(|| v.int_value.map(|n| n as f64))
                .or_else(|| v.uint_value.map(|n| n as f64))
                .or_else(|| v.sint_value.map(|n| n as f64));
        }
    }
    None
}

fn remove_feature_tag(tags: &mut Vec<u32>, key: u32) {
    let mut kept = Vec::with_capacity(tags.len());
    for pair in tags.as_chunks::<2>().0 {
        if pair[0] != key {
            kept.extend_from_slice(pair);
        }
    }
    *tags = kept;
}

fn set_feature_tag(tags: &mut [u32], key: u32, value_index: u32) {
    for pair in tags.as_chunks_mut::<2>().0 {
        if pair[0] == key {
            pair[1] = value_index;
            return;
        }
    }
}

fn decode_mvt_rings(geometry: &[u32]) -> Vec<Vec<[i32; 2]>> {
    let mut rings: Vec<Vec<[i32; 2]>> = Vec::new();
    let (mut cx, mut cy) = (0_i32, 0_i32);
    let mut idx = 0;
    while idx < geometry.len() {
        let cmd_int = geometry[idx];
        idx += 1;
        let (cmd, count) = (cmd_int & 7, (cmd_int >> 3) as usize);
        if cmd == 1 || cmd == 2 {
            for _ in 0..count {
                if idx + 1 >= geometry.len() {
                    return rings;
                }
                let raw_dx = geometry[idx];
                let raw_dy = geometry[idx + 1];
                idx += 2;
                let dx = ((raw_dx >> 1) as i32) ^ -((raw_dx & 1) as i32);
                let dy = ((raw_dy >> 1) as i32) ^ -((raw_dy & 1) as i32);
                cx = cx.saturating_add(dx);
                cy = cy.saturating_add(dy);
                if cmd == 1 {
                    rings.push(vec![[cx, cy]]);
                } else if let Some(ring) = rings.last_mut() {
                    ring.push([cx, cy]);
                }
            }
        }
    }
    rings
}

fn rings_bbox(rings: &[Vec<[i32; 2]>]) -> Option<[i32; 4]> {
    let mut iter = rings.iter().flatten();
    let &[x0, y0] = iter.next()?;
    let mut bbox = [x0, y0, x0, y0];
    for &[x, y] in iter {
        bbox[0] = bbox[0].min(x);
        bbox[1] = bbox[1].min(y);
        bbox[2] = bbox[2].max(x);
        bbox[3] = bbox[3].max(y);
    }
    Some(bbox)
}

fn point_in_mvt_ring(ring: &[[i32; 2]], [x, y]: [f32; 2]) -> bool {
    let mut inside = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let (ax, ay) = (a[0] as f32, a[1] as f32);
        let (bx, by) = (b[0] as f32, b[1] as f32);
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
    }
    inside
}

//! In-place GPU metadata fast path for stationary dynamic-point GeoJSON circle sources.
//!
//! When a GeoJSON `FeatureCollection` of `Point`s keeps identical feature IDs and coordinates
//! across updates (such as real-time sensor telemetry), this module parses incoming UTF-8 JSON
//! directly into [`FeatureProperties`] without `serde_json::Value` DOM allocation, skips bumping
//! `source.generation` / `style.revision()` (zero worker re-tiling), and updates only visible
//! tiles' `[fill_rgba8, packed_radius_f16x2]` GPU feature metadata in-place.

mod gpu;
mod wire;

use std::sync::Arc;

pub(crate) use gpu::{
    ensure_seeded_for_style, populate_dynamic_circle_bucket, record_uploaded_dynamic_circle,
    refresh_dynamic_point_layers,
};
use rustc_hash::FxHashMap;
use serde_json::Value as JsonValue;
use wire::{fill_properties_from_json, project_lon_lat, try_parse_stationary_bytes, RawFeatureId};

use super::promoted_property;
use crate::{
    coords::WorldTileCoords,
    geojson::index::{coverage::TileWindow, EXTENT},
    render::shaders::FillShaderFeatureMetadata,
    style::{
        expression::{reads_identity, FeatureProperties},
        feature_state::{layer_geometry_reads_feature_state, layer_paint_reads_feature_state},
        layer::{LayerPaint, StyleLayer, StyleProperty},
        source::{GeoJsonData, GeoJsonSource, Source},
        Style, StyleLayerId,
    },
    tcs::world::World,
};

/// World resource tracking stationary `Point` GeoJSON sources for zero-retiling GPU updates.
#[derive(Default)]
pub(crate) struct DynamicPointGeoJson {
    last_style_revision: Option<u64>,
    sources: FxHashMap<String, DynamicPointSource>,
    gpu_scratch: Vec<FillShaderFeatureMetadata>,
}

struct DynamicPointSource {
    keys: Vec<(RawFeatureId, [u64; 2])>,
    world_points: Vec<[f64; 2]>,
    properties: Vec<FeatureProperties>,
    scratch_props: Vec<FeatureProperties>,
    promoted: Option<String>,
    generate_id: bool,
    inject_identity: bool,
    seq: u64,
    tile_hits: FxHashMap<WorldTileCoords, Vec<u32>>,
    applied_seq: FxHashMap<(WorldTileCoords, StyleLayerId), (u64, u64)>,
    deferred_bytes: Option<Arc<[u8]>>,
}

fn compute_tile_hits(world_points: &[[f64; 2]], coords: WorldTileCoords) -> Vec<u32> {
    let TileWindow {
        tiles,
        west,
        north,
        bounds,
    } = TileWindow::of(coords);
    let extent = f64::from(EXTENT);
    let extent_i32 = EXTENT as i32;
    let mut hits = Vec::new();
    for (idx, pt) in world_points.iter().enumerate() {
        if !(bounds[0]..=bounds[2]).contains(&pt[0]) || !(bounds[1]..=bounds[3]).contains(&pt[1]) {
            continue;
        }
        let tx = ((pt[0] - west) * tiles * extent).round() as i32;
        let ty = ((pt[1] - north) * tiles * extent).round() as i32;
        if (0..extent_i32).contains(&tx) && (0..extent_i32).contains(&ty) {
            hits.push(idx as u32);
        }
    }
    hits
}

/// Returns true when `style_layer` uses [`CircleDynamicRadiusShader`] (`2 * vertices` metadata).
pub(crate) fn uses_dynamic_circle_metadata(style: &Style, style_layer: &StyleLayer) -> bool {
    let Some(source_name) = style_layer.source.as_deref() else {
        return false;
    };
    layer_supports_dynamic_points(style_layer) && source_supports_dynamic_points(style, source_name)
}

/// Returns true when `source_name` is an unclustered, unfiltered GeoJSON source whose referencing
/// style layers all support in-place dynamic circle metadata updates.
pub(crate) fn source_supports_dynamic_points(style: &Style, source_name: &str) -> bool {
    let Some(Source::GeoJson(src)) = style.sources.get(source_name) else {
        return false;
    };
    if src.cluster || src.filter.is_some() || src.line_metrics {
        return false;
    }
    let mut has_any_layer = false;
    for layer in &style.layers {
        if layer.source.as_deref() != Some(source_name) {
            continue;
        }
        has_any_layer = true;
        if !layer_supports_dynamic_points(layer) {
            return false;
        }
    }
    has_any_layer
}

fn layer_supports_dynamic_points(style_layer: &StyleLayer) -> bool {
    if style_layer.type_ != "circle"
        || style_layer.filter.is_some()
        || style_layer
            .unrecognized
            .layout
            .contains_key("circle-sort-key")
        || layer_paint_reads_feature_state(style_layer)
        || layer_geometry_reads_feature_state(style_layer)
    {
        return false;
    }
    let Some(LayerPaint::Circle(paint)) = style_layer.paint.as_ref() else {
        return false;
    };
    let radius_varies = !paint.radius().is_feature_constant();
    let color_varies = paint
        .circle_color
        .as_ref()
        .is_some_and(|c| !c.is_feature_constant());
    let opacity_varies = paint
        .circle_opacity
        .as_ref()
        .is_some_and(|o| !o.is_feature_constant());
    if !(radius_varies || color_varies || opacity_varies) {
        return false;
    }
    paint.stroke_color_per_feature().is_none()
        && paint
            .circle_stroke_width
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && paint
            .circle_blur
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && paint
            .circle_stroke_opacity
            .as_ref()
            .is_none_or(StyleProperty::is_feature_constant)
        && !prop_reads_spatial(paint.circle_radius.as_ref())
        && !prop_reads_spatial(paint.circle_color.as_ref())
        && !prop_reads_spatial(paint.circle_opacity.as_ref())
}

fn prop_reads_spatial<T>(prop: Option<&StyleProperty<T>>) -> bool {
    match prop {
        Some(StyleProperty::Expression(expr)) => json_reads_spatial(expr.source()),
        _ => false,
    }
}

fn json_reads_spatial(value: &JsonValue) -> bool {
    match value {
        JsonValue::Array(items) => {
            matches!(
                items.first().and_then(JsonValue::as_str),
                Some("within" | "distance")
            ) || items.iter().any(json_reads_spatial)
        }
        JsonValue::Object(map) => map.values().any(json_reads_spatial),
        _ => false,
    }
}

fn source_layers_read_identity(style: &Style, source_name: &str) -> bool {
    style.layers.iter().any(|layer| {
        layer.source.as_deref() == Some(source_name)
            && match layer.paint.as_ref() {
                Some(LayerPaint::Circle(p)) => {
                    prop_reads_identity(p.circle_radius.as_ref())
                        || prop_reads_identity(p.circle_color.as_ref())
                        || prop_reads_identity(p.circle_opacity.as_ref())
                }
                _ => false,
            }
    })
}

fn prop_reads_identity<T>(prop: Option<&StyleProperty<T>>) -> bool {
    match prop {
        Some(StyleProperty::Expression(expr)) => reads_identity(expr.source()),
        _ => false,
    }
}

/// Materializes any deferred stationary `bytes` back into `GeoJsonSource::data` before a
/// non-stationary style/source mutation or diff.
pub(crate) fn flush_deferred_geojson_bytes(style: &mut Style, world: &mut World) {
    let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() else {
        return;
    };
    for (name, src_state) in &mut state.sources {
        let Some(bytes) = src_state.deferred_bytes.take() else {
            continue;
        };
        if let Some(Source::GeoJson(src)) = style.sources.get_mut(name) {
            if let Ok(doc) = serde_json::from_slice::<JsonValue>(&bytes) {
                src.data = GeoJsonData::Inline(Arc::new(doc));
            }
        }
    }
}

/// Invalidates `applied_seq` across tracked dynamic sources when style paint or layer order edits
/// occur so resident `CircleDynamicRadiusShader` allocations re-evaluate in-place on next frame.
pub(crate) fn invalidate_after_style_change(style: &Style, world: &mut World) {
    let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() else {
        return;
    };
    state.last_style_revision = None;
    state
        .sources
        .retain(|name, _| source_supports_dynamic_points(style, name));
    for (name, src_state) in &mut state.sources {
        src_state.inject_identity = source_layers_read_identity(style, name);
        src_state.seq = src_state.seq.wrapping_add(1);
        src_state.applied_seq.clear();
    }
}

/// Parses deferred stationary UTF-8 bytes on demand for read-only [`MapContext::query_source_features`].
pub(crate) fn deferred_inline_value(world: &World, source_name: &str) -> Option<JsonValue> {
    let bytes = world
        .resources
        .get::<DynamicPointGeoJson>()?
        .sources
        .get(source_name)?
        .deferred_bytes
        .as_ref()?;
    serde_json::from_slice(bytes).ok()
}

/// Seeds or removes `source_name` in [`DynamicPointGeoJson`] from `style`'s current inline data.
pub(crate) fn seed_from_inline(style: &Style, world: &mut World, source_name: &str) {
    if !source_supports_dynamic_points(style, source_name) {
        if let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() {
            state.sources.remove(source_name);
        }
        return;
    }
    let Some(Source::GeoJson(src)) = style.sources.get(source_name) else {
        return;
    };
    let GeoJsonData::Inline(doc) = &src.data else {
        if let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() {
            state.sources.remove(source_name);
        }
        return;
    };
    let inject_identity = source_layers_read_identity(style, source_name);
    let Some(built) = build_source_from_value(doc, src, inject_identity) else {
        if let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() {
            state.sources.remove(source_name);
        }
        return;
    };
    let state = world.resources.get_or_init_mut::<DynamicPointGeoJson>();
    state.sources.insert(source_name.to_owned(), built);
}

fn build_source_from_value(
    doc: &JsonValue,
    src: &GeoJsonSource,
    inject_identity: bool,
) -> Option<DynamicPointSource> {
    if doc.get("type").and_then(JsonValue::as_str) != Some("FeatureCollection") {
        return None;
    }
    let features = doc.get("features").and_then(JsonValue::as_array)?;
    if features.is_empty() {
        return None;
    }
    let promoted = promoted_property(src);
    let n = features.len();
    let mut keys = Vec::with_capacity(n);
    let mut world_points = Vec::with_capacity(n);
    let mut properties = Vec::with_capacity(n);

    for (idx, feature) in features.iter().enumerate() {
        if feature.get("type").and_then(JsonValue::as_str) != Some("Feature") {
            return None;
        }
        let geom = feature.get("geometry")?;
        if geom.get("type").and_then(JsonValue::as_str) != Some("Point") {
            return None;
        }
        let coords = geom.get("coordinates").and_then(JsonValue::as_array)?;
        let lon = coords.first().and_then(JsonValue::as_f64)?;
        let lat = coords.get(1).and_then(JsonValue::as_f64)?;
        let projected = project_lon_lat(lon, lat)?;
        let raw_id = RawFeatureId::from_json(feature.get("id"));
        let mut props = FeatureProperties::new();
        fill_properties_from_json(
            &mut props,
            feature.get("properties"),
            &raw_id,
            promoted.as_deref(),
            src.generate_id.then_some(idx),
            inject_identity,
        );
        keys.push((raw_id, [lon.to_bits(), lat.to_bits()]));
        world_points.push(projected);
        properties.push(props);
    }

    let scratch_props = properties.clone();
    Some(DynamicPointSource {
        keys,
        world_points,
        properties,
        scratch_props,
        promoted,
        generate_id: src.generate_id,
        inject_identity,
        seq: 1,
        tile_hits: FxHashMap::default(),
        applied_seq: FxHashMap::default(),
        deferred_bytes: None,
    })
}

/// Attempts a zero-DOM-allocation stationary update from UTF-8 JSON `bytes`.
pub(crate) fn try_apply_stationary_bytes(
    style: &Style,
    world: &mut World,
    source_name: &str,
    bytes: &[u8],
) -> bool {
    if !source_supports_dynamic_points(style, source_name) {
        return false;
    }
    let needs_seed = world
        .resources
        .get::<DynamicPointGeoJson>()
        .is_none_or(|s| !s.sources.contains_key(source_name));
    if needs_seed {
        seed_from_inline(style, world, source_name);
    }
    let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() else {
        return false;
    };
    let Some(src_state) = state.sources.get_mut(source_name) else {
        return false;
    };
    if !try_parse_stationary_bytes(
        bytes,
        &src_state.keys,
        &mut src_state.scratch_props,
        src_state.promoted.as_deref(),
        src_state.generate_id,
        src_state.inject_identity,
    ) {
        return false;
    }
    std::mem::swap(&mut src_state.properties, &mut src_state.scratch_props);
    src_state.seq = src_state.seq.wrapping_add(1);
    src_state.deferred_bytes = Some(Arc::from(bytes));
    crate::render::frame_signals::mark_dirty(world);
    true
}

/// Attempts an in-place stationary update from an inline `serde_json::Value` document.
pub(crate) fn try_apply_stationary_inline(
    style: &mut Style,
    world: &mut World,
    source_name: &str,
    doc: &Arc<JsonValue>,
) -> bool {
    if !source_supports_dynamic_points(style, source_name) {
        return false;
    }
    let needs_seed = world
        .resources
        .get::<DynamicPointGeoJson>()
        .is_none_or(|s| !s.sources.contains_key(source_name));
    if needs_seed {
        seed_from_inline(style, world, source_name);
    }
    let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() else {
        return false;
    };
    let Some(src_state) = state.sources.get_mut(source_name) else {
        return false;
    };
    if doc.get("type").and_then(JsonValue::as_str) != Some("FeatureCollection") {
        return false;
    }
    let Some(features) = doc.get("features").and_then(JsonValue::as_array) else {
        return false;
    };
    if features.len() != src_state.keys.len() || features.is_empty() {
        return false;
    }
    for (idx, (feature, expected)) in features.iter().zip(src_state.keys.iter()).enumerate() {
        if feature.get("type").and_then(JsonValue::as_str) != Some("Feature") {
            return false;
        }
        let Some(geom) = feature.get("geometry") else {
            return false;
        };
        if geom.get("type").and_then(JsonValue::as_str) != Some("Point") {
            return false;
        }
        let Some(coords) = geom.get("coordinates").and_then(JsonValue::as_array) else {
            return false;
        };
        let (Some(lon), Some(lat)) = (
            coords.first().and_then(JsonValue::as_f64),
            coords.get(1).and_then(JsonValue::as_f64),
        ) else {
            return false;
        };
        if !lon.is_finite()
            || !lat.is_finite()
            || [lon.to_bits(), lat.to_bits()] != expected.1
            || RawFeatureId::from_json(feature.get("id")) != expected.0
        {
            return false;
        }
        fill_properties_from_json(
            &mut src_state.scratch_props[idx],
            feature.get("properties"),
            &expected.0,
            src_state.promoted.as_deref(),
            src_state.generate_id.then_some(idx),
            src_state.inject_identity,
        );
    }
    std::mem::swap(&mut src_state.properties, &mut src_state.scratch_props);
    src_state.seq = src_state.seq.wrapping_add(1);
    src_state.deferred_bytes = None;
    if let Some(Source::GeoJson(src)) = style.sources.get_mut(source_name) {
        src.data = GeoJsonData::Inline(Arc::clone(doc));
    }
    crate::render::frame_signals::mark_dirty(world);
    true
}

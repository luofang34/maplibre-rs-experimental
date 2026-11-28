//! In-place GPU metadata fast path for stationary dynamic-point GeoJSON circle sources.
//!
//! When a GeoJSON `FeatureCollection` of `Point`s keeps identical feature IDs and coordinates
//! across updates (such as real-time sensor telemetry), this module parses incoming UTF-8 JSON
//! directly into [`FeatureProperties`] without `serde_json::Value` DOM allocation, skips bumping
//! `source.generation` / `style.revision()` (zero worker re-tiling), and updates only visible
//! tiles' `[fill_rgba8, packed_radius_f16x2]` GPU feature metadata in-place.

mod eligibility;
mod fast_scan;
mod gpu;
mod ramp;
mod wire;

use std::{borrow::Cow, sync::Arc};

use eligibility::source_layers_read_identity;
pub(crate) use eligibility::{source_supports_dynamic_points, uses_dynamic_circle_metadata};
use fast_scan::{collect_fast_ramp_keys, try_fast_scan_stationary_bytes, FastColumnStore};
pub(crate) use gpu::{
    ensure_seeded_for_style, populate_dynamic_circle_bucket, record_uploaded_dynamic_circle,
    refresh_dynamic_point_layers, DynamicCircleBucketInput,
};
use rustc_hash::FxHashMap;
use serde_json::Value as JsonValue;
use wire::{fill_properties_from_json, project_lon_lat, try_parse_stationary_bytes, RawFeatureId};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    render::shaders::FillShaderFeatureMetadata,
    style::{
        expression::FeatureProperties,
        source::{
            promoted_geojson_property as promoted_property, GeoJsonData, GeoJsonSource, Source,
        },
        Style, StyleLayerId,
    },
    tcs::{tile_window::TileWindow, world::World},
};

/// World resource tracking stationary `Point` GeoJSON sources for zero-retiling GPU updates.
#[derive(Default)]
pub(crate) struct DynamicPointGeoJson {
    last_style_revision: Option<u64>,
    sources: FxHashMap<String, DynamicPointSource>,
    circle_layers: Vec<(usize, String)>,
    gpu_scratch: Vec<FillShaderFeatureMetadata>,
}

#[derive(Default)]
pub(super) struct EvalCache {
    pub key: Option<(u64, StyleLayerId, u64)>,
    pub ramps: Option<gpu::eval::CompiledCircleRamps>,
    pub pairs: Vec<[FillShaderFeatureMetadata; 2]>,
    pub prev_key: Option<(u64, StyleLayerId, u64)>,
    pub prev_pairs: Vec<[FillShaderFeatureMetadata; 2]>,
    pub any_changed: bool,
}

type SlotCacheEntry = Option<(usize, u8, u64, usize)>;

impl DynamicPointGeoJson {
    /// Drops cached tile hits and slot states for an evicted world tile.
    pub(crate) fn evict_tile(&mut self, coords: WorldTileCoords) {
        for src in self.sources.values_mut() {
            src.tile_hits.remove(&coords);
            src.applied_seq.retain(|(c, _), _| *c != coords);
            src.slot_cache.retain(|(c, _), _| *c != coords);
        }
    }
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
    slot_cache_state: Option<(u64, u64)>,
    slot_cache: FxHashMap<(WorldTileCoords, StyleLayerId), SlotCacheEntry>,
    deferred_bytes: Option<Vec<u8>>,
    fast_ramp_keys_rev: Option<u64>,
    fast_ramp_keys_ok: bool,
    pub(super) fast_columns: FastColumnStore,
    eval_cache: EvalCache,
    clean_frame: Option<(u64, (u64, u64), Vec<WorldTileCoords>)>,
}

impl DynamicPointSource {
    pub(super) fn matches_clean_frame(
        &self,
        pool_state: (u64, u64),
        painted_tiles: &[WorldTileCoords],
    ) -> bool {
        self.clean_frame.as_ref().is_some_and(|(s, ps, tiles)| {
            *s == self.seq && *ps == pool_state && tiles.as_slice() == painted_tiles
        })
    }

    pub(super) fn set_clean_frame(
        &mut self,
        pool_state: (u64, u64),
        painted_tiles: &[WorldTileCoords],
        clean: bool,
    ) {
        if !clean {
            self.clean_frame = None;
            return;
        }
        let mut buf = self
            .clean_frame
            .take()
            .map(|(_, _, v)| v)
            .unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(painted_tiles);
        self.clean_frame = Some((self.seq, pool_state, buf));
    }

    fn ensure_scratch_props(&mut self) {
        if self.scratch_props.len() != self.keys.len() {
            self.scratch_props
                .resize_with(self.keys.len(), FeatureProperties::default);
        }
    }
}

fn compute_tile_hits(world_points: &[[f64; 2]], coords: WorldTileCoords) -> Vec<u32> {
    let TileWindow {
        tiles,
        west,
        north,
        bounds,
    } = TileWindow::of(coords);
    let extent = EXTENT;
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
        if src_state.fast_columns.active_seq == Some(src_state.seq) {
            if let Some(bytes) = src_state.deferred_bytes.as_deref() {
                if src_state.scratch_props.len() != src_state.keys.len() {
                    src_state
                        .scratch_props
                        .resize_with(src_state.keys.len(), FeatureProperties::default);
                }
                if try_parse_stationary_bytes(
                    bytes,
                    &src_state.keys,
                    &mut src_state.scratch_props,
                    src_state.promoted.as_deref(),
                    src_state.generate_id,
                    src_state.inject_identity,
                ) {
                    std::mem::swap(&mut src_state.properties, &mut src_state.scratch_props);
                }
            }
            src_state.fast_columns.active_seq = None;
        }
        src_state.inject_identity = source_layers_read_identity(style, name);
        src_state.seq = src_state.seq.wrapping_add(1);
        src_state.applied_seq.clear();
        src_state.fast_ramp_keys_rev = None;
        src_state.eval_cache.ramps = None;
    }
}

/// Parses deferred stationary UTF-8 bytes on demand for read-only [`MapContext::query_source_features`].
pub(crate) fn deferred_inline_value(world: &World, source_name: &str) -> Option<JsonValue> {
    let src = world
        .resources
        .get::<DynamicPointGeoJson>()?
        .sources
        .get(source_name)?;
    src.deferred_bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
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
        let mut props = FeatureProperties::default();
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

    Some(DynamicPointSource {
        keys,
        world_points,
        properties,
        scratch_props: Vec::new(),
        promoted,
        generate_id: src.generate_id,
        inject_identity,
        seq: 1,
        tile_hits: FxHashMap::default(),
        applied_seq: FxHashMap::default(),
        slot_cache_state: None,
        slot_cache: FxHashMap::default(),
        deferred_bytes: None,
        fast_ramp_keys_rev: None,
        fast_ramp_keys_ok: false,
        fast_columns: FastColumnStore::default(),
        eval_cache: EvalCache::default(),
        clean_frame: None,
    })
}

fn store_deferred_bytes(slot: &mut Option<Vec<u8>>, bytes: Cow<'_, [u8]>) {
    match bytes {
        Cow::Owned(owned) => *slot = Some(owned),
        Cow::Borrowed(borrowed) => {
            let buf = slot.get_or_insert_with(Vec::new);
            buf.clear();
            buf.extend_from_slice(borrowed);
        }
    }
}

/// Attempts a zero-DOM-allocation stationary update from UTF-8 JSON `bytes`, moving owned buffers
/// directly without copying on success and returning the buffer back on `Err` otherwise.
pub(crate) fn try_apply_stationary_bytes<'a>(
    style: &Style,
    world: &mut World,
    source_name: &str,
    bytes: Cow<'a, [u8]>,
) -> Result<(), Cow<'a, [u8]>> {
    if !source_supports_dynamic_points(style, source_name) {
        return Err(bytes);
    }
    let needs_seed = world
        .resources
        .get::<DynamicPointGeoJson>()
        .is_none_or(|s| !s.sources.contains_key(source_name));
    if needs_seed {
        seed_from_inline(style, world, source_name);
    }
    let Some(state) = world.resources.get_mut::<DynamicPointGeoJson>() else {
        return Err(bytes);
    };
    let Some(src_state) = state.sources.get_mut(source_name) else {
        return Err(bytes);
    };
    let style_rev = style.revision();
    let fast_eligible = if src_state.fast_ramp_keys_rev == Some(style_rev) {
        src_state.fast_ramp_keys_ok
    } else {
        let ok = !src_state.inject_identity
            && src_state.promoted.is_none()
            && collect_fast_ramp_keys(style, source_name, &mut src_state.fast_columns.keys);
        src_state.fast_ramp_keys_rev = Some(style_rev);
        src_state.fast_ramp_keys_ok = ok;
        ok
    };
    if fast_eligible
        && try_fast_scan_stationary_bytes(&bytes, &src_state.keys, &mut src_state.fast_columns)
    {
        src_state.seq = src_state.seq.wrapping_add(1);
        src_state.fast_columns.active_seq = Some(src_state.seq);
        store_deferred_bytes(&mut src_state.deferred_bytes, bytes);
        crate::render::frame_signals::mark_dirty(world);
        return Ok(());
    }
    src_state.fast_columns.active_seq = None;
    src_state.ensure_scratch_props();
    if !try_parse_stationary_bytes(
        &bytes,
        &src_state.keys,
        &mut src_state.scratch_props,
        src_state.promoted.as_deref(),
        src_state.generate_id,
        src_state.inject_identity,
    ) {
        return Err(bytes);
    }
    std::mem::swap(&mut src_state.properties, &mut src_state.scratch_props);
    src_state.seq = src_state.seq.wrapping_add(1);
    store_deferred_bytes(&mut src_state.deferred_bytes, bytes);
    crate::render::frame_signals::mark_dirty(world);
    Ok(())
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
    src_state.ensure_scratch_props();
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
        let Some(slot) = src_state.scratch_props.get_mut(idx) else {
            return false;
        };
        fill_properties_from_json(
            slot,
            feature.get("properties"),
            &expected.0,
            src_state.promoted.as_deref(),
            src_state.generate_id.then_some(idx),
            src_state.inject_identity,
        );
    }
    std::mem::swap(&mut src_state.properties, &mut src_state.scratch_props);
    src_state.seq = src_state.seq.wrapping_add(1);
    src_state.fast_columns.active_seq = None;
    src_state.deferred_bytes = None;
    if let Some(Source::GeoJson(src)) = style.sources.get_mut(source_name) {
        src.data = GeoJsonData::Inline(Arc::clone(doc));
    }
    crate::render::frame_signals::mark_dirty(world);
    true
}

//! Zero-allocation UTF-8 JSON byte scanner populating columnar ramp inputs in-place.

mod cursor;

use cursor::{parse_u64_digits, Scanner};

use super::{
    ramp::{ColorRamp, ScalarRamp},
    wire::RawFeatureId,
};
use crate::style::{layer::LayerPaint, Style};

pub(super) const MAX_FAST_COLS: usize = 4;

#[derive(Default)]
pub(super) struct FastColumnStore {
    pub active_seq: Option<u64>,
    pub keys: Vec<String>,
    pub values: Vec<[Option<f64>; MAX_FAST_COLS]>,
    coord_bytes: Vec<u8>,
    coord_spans: Vec<(u32, u16)>,
}

/// Returns `true` when every circle layer on `source_name` reads at most [`MAX_FAST_COLS`] numeric
/// properties exclusively via compiled [`ScalarRamp`] / [`ColorRamp`] expressions.
pub(super) fn collect_fast_ramp_keys(
    style: &Style,
    source_name: &str,
    out_keys: &mut Vec<String>,
) -> bool {
    out_keys.clear();
    let mut found_layer = false;
    for layer in &style.layers {
        if layer.source.as_deref() != Some(source_name) {
            continue;
        }
        found_layer = true;
        if layer.type_ != "circle" {
            return false;
        }
        let Some(LayerPaint::Circle(paint)) = layer.paint.as_ref() else {
            return false;
        };
        let radius = paint.radius();
        if !radius.is_feature_constant() {
            let Some(ramp) = ScalarRamp::compile(&radius) else {
                return false;
            };
            if !push_key(out_keys, ramp.key()) {
                return false;
            }
        }
        if let Some(color_prop) = paint.circle_color.as_ref() {
            if !color_prop.is_feature_constant() {
                let Some(ramp) = ColorRamp::compile(color_prop) else {
                    return false;
                };
                if !push_key(out_keys, ramp.key()) {
                    return false;
                }
            }
        }
        if let Some(opacity_prop) = paint.circle_opacity.as_ref() {
            if !opacity_prop.is_feature_constant() {
                let Some(ramp) = ScalarRamp::compile(opacity_prop) else {
                    return false;
                };
                if !push_key(out_keys, ramp.key()) {
                    return false;
                }
            }
        }
    }
    found_layer && !out_keys.is_empty() && out_keys.len() <= MAX_FAST_COLS
}

fn push_key(out_keys: &mut Vec<String>, key: &str) -> bool {
    if key.contains('\\') {
        return false;
    }
    if out_keys.iter().any(|existing| existing == key) {
        return true;
    }
    if out_keys.len() >= MAX_FAST_COLS {
        return false;
    }
    out_keys.push(key.to_owned());
    true
}

/// Scans `bytes` directly without heap allocations after warm-up, updating `store.values` when
/// every feature preserves `expected_keys` (`id` + `[lon, lat]`).
pub(super) fn try_fast_scan_stationary_bytes(
    bytes: &[u8],
    expected_keys: &[(RawFeatureId, [u64; 2])],
    store: &mut FastColumnStore,
) -> bool {
    if expected_keys.is_empty() || store.keys.is_empty() || store.keys.len() > MAX_FAST_COLS {
        return false;
    }
    if store.values.len() != expected_keys.len() {
        store
            .values
            .resize(expected_keys.len(), [None; MAX_FAST_COLS]);
    }
    let cache_ready = store.coord_spans.len() == expected_keys.len();
    if !cache_ready {
        store.coord_bytes.clear();
        store.coord_spans.clear();
        store.coord_spans.resize(expected_keys.len(), (0, 0));
    }

    let mut s = Scanner::new(bytes);
    if !scan_collection(&mut s, expected_keys, store) {
        if !cache_ready {
            store.coord_spans.clear();
            store.coord_bytes.clear();
        }
        return false;
    }
    s.skip_ws();
    s.pos == bytes.len()
}

fn scan_collection(
    s: &mut Scanner<'_>,
    expected_keys: &[(RawFeatureId, [u64; 2])],
    store: &mut FastColumnStore,
) -> bool {
    if !s.consume(b'{') {
        return false;
    }
    let mut is_fc = false;
    let mut features_ok = false;
    s.skip_ws();
    if s.bytes.get(s.pos) == Some(&b'}') {
        return false;
    }
    loop {
        let Some(key) = s.next_simple_str() else {
            return false;
        };
        if !s.consume(b':') {
            return false;
        }
        match key {
            "type" => {
                if s.next_simple_str() != Some("FeatureCollection") {
                    return false;
                }
                is_fc = true;
            }
            "features" => {
                if features_ok || !scan_features(s, expected_keys, store) {
                    return false;
                }
                features_ok = true;
            }
            _ => {
                if !s.skip_value() {
                    return false;
                }
            }
        }
        if s.consume(b',') {
            continue;
        }
        if s.consume(b'}') {
            return is_fc && features_ok;
        }
        return false;
    }
}

fn scan_features(
    s: &mut Scanner<'_>,
    expected_keys: &[(RawFeatureId, [u64; 2])],
    store: &mut FastColumnStore,
) -> bool {
    if !s.consume(b'[') {
        return false;
    }
    for (idx, expected) in expected_keys.iter().enumerate() {
        if idx > 0 && !s.consume(b',') {
            return false;
        }
        if !scan_feature(s, idx, &expected.0, expected.1, store) {
            return false;
        }
    }
    s.consume(b']')
}

fn scan_feature(
    s: &mut Scanner<'_>,
    idx: usize,
    expected_id: &RawFeatureId,
    expected_coords: [u64; 2],
    store: &mut FastColumnStore,
) -> bool {
    if !s.consume(b'{') {
        return false;
    }
    let mut is_feature = false;
    let mut geom_ok = false;
    let mut id_ok = matches!(expected_id, RawFeatureId::None);
    store.values[idx] = [None; MAX_FAST_COLS];

    loop {
        let Some(key) = s.next_simple_str() else {
            return false;
        };
        if !s.consume(b':') {
            return false;
        }
        match key {
            "type" => {
                if s.next_simple_str() != Some("Feature") {
                    return false;
                }
                is_feature = true;
            }
            "id" => {
                if !check_id(s, expected_id) {
                    return false;
                }
                id_ok = true;
            }
            "geometry" => {
                if geom_ok || !scan_point_geometry(s, idx, expected_coords, store) {
                    return false;
                }
                geom_ok = true;
            }
            "properties" => {
                if !scan_properties(s, idx, store) {
                    return false;
                }
            }
            _ => {
                if !s.skip_value() {
                    return false;
                }
            }
        }
        if s.consume(b',') {
            continue;
        }
        if s.consume(b'}') {
            return is_feature && geom_ok && id_ok;
        }
        return false;
    }
}

fn check_id(s: &mut Scanner<'_>, expected_id: &RawFeatureId) -> bool {
    s.skip_ws();
    match (s.bytes.get(s.pos), expected_id) {
        (Some(b'n'), RawFeatureId::None) => s.consume_lit(b"null"),
        (Some(b'"'), RawFeatureId::Other(expected)) => s
            .next_simple_str()
            .is_some_and(|val| val == expected.as_ref()),
        (Some(b'0'..=b'9'), RawFeatureId::UInt(expected)) => {
            let Some(tok) = s.scan_number_token() else {
                return false;
            };
            if let Some(id) = parse_u64_digits(tok) {
                return id == *expected;
            }
            std::str::from_utf8(tok)
                .ok()
                .and_then(|raw| raw.parse::<f64>().ok())
                .is_some_and(|n| {
                    n >= 0.0 && n.fract() == 0.0 && n < 9.0e15 && (n as u64) == *expected
                })
        }
        _ => false,
    }
}

fn scan_point_geometry(
    s: &mut Scanner<'_>,
    idx: usize,
    expected_coords: [u64; 2],
    store: &mut FastColumnStore,
) -> bool {
    if !s.consume(b'{') {
        return false;
    }
    let mut is_point = false;
    let mut coords_ok = false;
    loop {
        let Some(key) = s.next_simple_str() else {
            return false;
        };
        if !s.consume(b':') {
            return false;
        }
        match key {
            "type" => {
                if s.next_simple_str() != Some("Point") {
                    return false;
                }
                is_point = true;
            }
            "coordinates" => {
                if coords_ok || !check_coords(s, idx, expected_coords, store) {
                    return false;
                }
                coords_ok = true;
            }
            _ => {
                if !s.skip_value() {
                    return false;
                }
            }
        }
        if s.consume(b',') {
            continue;
        }
        if s.consume(b'}') {
            return is_point && coords_ok;
        }
        return false;
    }
}

fn check_coords(
    s: &mut Scanner<'_>,
    idx: usize,
    expected_coords: [u64; 2],
    store: &mut FastColumnStore,
) -> bool {
    if !s.consume(b'[') {
        return false;
    }
    let coords_start = s.pos;
    let (cached_off, cached_len) = store.coord_spans[idx];
    if cached_len > 0 {
        let off = cached_off as usize;
        let len = cached_len as usize;
        let end = coords_start + len;
        if end < s.bytes.len()
            && s.bytes.get(coords_start..end) == store.coord_bytes.get(off..off + len)
            && s.bytes.get(end) == Some(&b']')
        {
            s.pos = end + 1;
            return true;
        }
    }
    let Some(lon) = s.next_f64_exact() else {
        return false;
    };
    if !s.consume(b',') {
        return false;
    }
    let Some(lat) = s.next_f64_exact() else {
        return false;
    };
    let coords_end = s.pos;
    if !s.consume(b']') {
        return false;
    }
    if [lon.to_bits(), lat.to_bits()] != expected_coords {
        return false;
    }
    if cached_len == 0 {
        let slice = &s.bytes[coords_start..coords_end];
        if let (Ok(off), Ok(len)) = (
            u32::try_from(store.coord_bytes.len()),
            u16::try_from(slice.len()),
        ) {
            store.coord_bytes.extend_from_slice(slice);
            store.coord_spans[idx] = (off, len);
        }
    }
    true
}

fn scan_properties(s: &mut Scanner<'_>, idx: usize, store: &mut FastColumnStore) -> bool {
    s.skip_ws();
    if s.bytes.get(s.pos) == Some(&b'n') {
        return s.consume_lit(b"null");
    }
    if !s.consume(b'{') {
        return false;
    }
    s.skip_ws();
    if s.consume(b'}') {
        return true;
    }
    loop {
        let Some(key) = s.next_simple_str() else {
            return false;
        };
        let Some(col) = store.keys.iter().position(|k| k == key) else {
            return false;
        };
        if !s.consume(b':') {
            return false;
        }
        s.skip_ws();
        if s.bytes.get(s.pos) == Some(&b'n') {
            if !s.consume_lit(b"null") {
                return false;
            }
            store.values[idx][col] = None;
        } else {
            let Some(val) = s.next_f64_fast() else {
                return false;
            };
            store.values[idx][col] = Some(val);
        }
        if s.consume(b',') {
            continue;
        }
        return s.consume(b'}');
    }
}

//! Shared non-generic JSON object extraction helpers for direct `&Map<String, Value>` parsing.

use rustc_hash::FxHashMap as HashMap;
use serde::de;
use serde_json::{Map, Value};

#[inline(never)]
pub(crate) fn field_err(field: &str, msg: &str) -> serde_json::Error {
    de::Error::custom(format!("{field}: {msg}"))
}

#[inline(never)]
pub(crate) fn missing_field_err(field: &str) -> serde_json::Error {
    de::Error::custom(format!("missing field `{field}`"))
}

#[inline]
fn non_null<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|val| !val.is_null())
}

fn value_as_f64(value: &Value, field: &str) -> Result<f64, serde_json::Error> {
    value
        .as_f64()
        .ok_or_else(|| field_err(field, "expected number"))
}

fn require_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Value, serde_json::Error> {
    map.get(key).ok_or_else(|| missing_field_err(key))
}

fn expect_str<'a>(val: &'a Value, key: &str) -> Result<&'a str, serde_json::Error> {
    val.as_str()
        .ok_or_else(|| field_err(key, "expected string"))
}

fn expect_bool(val: &Value, key: &str) -> Result<bool, serde_json::Error> {
    val.as_bool()
        .ok_or_else(|| field_err(key, "expected boolean"))
}

fn expect_array<'a>(
    val: &'a Value,
    key: &str,
    msg: &str,
) -> Result<&'a [Value], serde_json::Error> {
    val.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| field_err(key, msg))
}

pub(crate) fn value_as_u64(value: &Value, field: &str) -> Result<u64, serde_json::Error> {
    value
        .as_u64()
        .ok_or_else(|| field_err(field, "expected non-negative integer"))
}

fn parse_u64_into<T: TryFrom<u64>>(
    val: &Value,
    key: &str,
    range_msg: &str,
) -> Result<T, serde_json::Error> {
    let raw = value_as_u64(val, key)?;
    T::try_from(raw).map_err(|_| field_err(key, range_msg))
}

fn parse_i32_coord(
    map: &Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<i32, serde_json::Error> {
    map.get(key)
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| field_err(label, "expected i32"))
}

fn parse_string_array(val: &Value, key: &str) -> Result<Vec<String>, serde_json::Error> {
    let items = expect_array(val, key, "expected array")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(field_err(key, "expected string element"));
        };
        out.push(text.to_owned());
    }
    Ok(out)
}

fn parse_value_map(val: &Value, key: &str) -> Result<HashMap<String, Value>, serde_json::Error> {
    let Some(obj) = val.as_object() else {
        return Err(field_err(key, "expected object"));
    };
    Ok(obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn parse_f64_quad(val: &Value, key: &str) -> Result<(f64, f64, f64, f64), serde_json::Error> {
    let [a, b, c, d] = expect_array(val, key, "expected bounds array")? else {
        return Err(field_err(key, "expected 4 numbers"));
    };
    Ok((
        value_as_f64(a, key)?,
        value_as_f64(b, key)?,
        value_as_f64(c, key)?,
        value_as_f64(d, key)?,
    ))
}

fn parse_coords_quad(val: &Value, key: &str) -> Result<[[f64; 2]; 4], serde_json::Error> {
    let [p0, p1, p2, p3] = expect_array(val, key, "expected 4 coordinates")? else {
        return Err(field_err(key, "expected 4 coordinates"));
    };
    Ok([
        json_f64_pair(p0, key)?,
        json_f64_pair(p1, key)?,
        json_f64_pair(p2, key)?,
        json_f64_pair(p3, key)?,
    ])
}

pub(crate) fn json_opt_str<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, serde_json::Error> {
    non_null(map, key)
        .map(|val| expect_str(val, key))
        .transpose()
}

pub(crate) fn json_opt_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, serde_json::Error> {
    Ok(json_opt_str(map, key)?.map(str::to_owned))
}

pub(crate) fn json_req_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<String, serde_json::Error> {
    expect_str(require_field(map, key)?, key).map(str::to_owned)
}

pub(crate) fn json_bool_or(
    map: &Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(default);
    };
    expect_bool(val, key)
}

pub(crate) fn json_opt_bool(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, serde_json::Error> {
    non_null(map, key)
        .map(|val| expect_bool(val, key))
        .transpose()
}

pub(crate) fn json_opt_f64(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, serde_json::Error> {
    non_null(map, key)
        .map(|val| value_as_f64(val, key))
        .transpose()
}

pub(crate) fn json_f64_or(
    map: &Map<String, Value>,
    key: &str,
    default: f64,
) -> Result<f64, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(default);
    };
    value_as_f64(val, key)
}

pub(crate) fn json_opt_u8(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u8>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_u64_into(val, key, "u8 out of range"))
        .transpose()
}

pub(crate) fn json_opt_u32(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u32>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_u64_into(val, key, "u32 out of range"))
        .transpose()
}

pub(crate) fn json_opt_u64(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, serde_json::Error> {
    non_null(map, key)
        .map(|val| value_as_u64(val, key))
        .transpose()
}

pub(crate) fn json_opt_usize(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<usize>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_u64_into(val, key, "usize out of range"))
        .transpose()
}

pub(crate) fn json_opt_string_vec(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_string_array(val, key))
        .transpose()
}

pub(crate) fn json_opt_value_map(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<HashMap<String, Value>>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_value_map(val, key))
        .transpose()
}

fn json_f64_pair(value: &Value, field: &str) -> Result<[f64; 2], serde_json::Error> {
    let [a, b] = expect_array(value, field, "expected [f64; 2]")? else {
        return Err(field_err(field, "expected 2 numbers"));
    };
    Ok([value_as_f64(a, field)?, value_as_f64(b, field)?])
}

pub(crate) fn json_opt_f64_pair(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<[f64; 2]>, serde_json::Error> {
    non_null(map, key)
        .map(|val| json_f64_pair(val, key))
        .transpose()
}

pub(crate) fn json_opt_bounds4(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<(f64, f64, f64, f64)>, serde_json::Error> {
    non_null(map, key)
        .map(|val| parse_f64_quad(val, key))
        .transpose()
}

pub(crate) fn json_req_coords4(
    map: &Map<String, Value>,
    key: &str,
) -> Result<[[f64; 2]; 4], serde_json::Error> {
    parse_coords_quad(require_field(map, key)?, key)
}

pub(crate) fn json_req_coords(
    map: &Map<String, Value>,
    key: &str,
) -> Result<crate::coords::WorldTileCoords, serde_json::Error> {
    let coords_map = map
        .get(key)
        .and_then(|v| v.as_object())
        .ok_or_else(|| missing_field_err(key))?;
    let x = parse_i32_coord(coords_map, "x", "coords.x")?;
    let y = parse_i32_coord(coords_map, "y", "coords.y")?;
    let z = json_opt_u8(coords_map, "z")?.ok_or_else(|| missing_field_err("coords.z"))?;
    Ok(crate::coords::WorldTileCoords {
        x,
        y,
        z: crate::coords::ZoomLevel::new(z),
    })
}

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

pub(crate) fn value_as_f64(value: &Value, field: &str) -> Result<f64, serde_json::Error> {
    value
        .as_f64()
        .ok_or_else(|| field_err(field, "expected number"))
}

pub(crate) fn value_as_u64(value: &Value, field: &str) -> Result<u64, serde_json::Error> {
    value
        .as_u64()
        .ok_or_else(|| field_err(field, "expected non-negative integer"))
}

pub(crate) fn json_opt_str<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_str()
        .map(Some)
        .ok_or_else(|| field_err(key, "expected string"))
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
    let Some(val) = map.get(key) else {
        return Err(missing_field_err(key));
    };
    val.as_str()
        .map(str::to_owned)
        .ok_or_else(|| field_err(key, "expected string"))
}

pub(crate) fn json_bool_or(
    map: &Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(default);
    };
    val.as_bool()
        .ok_or_else(|| field_err(key, "expected boolean"))
}

pub(crate) fn json_opt_bool(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_bool()
        .map(Some)
        .ok_or_else(|| field_err(key, "expected boolean"))
}

pub(crate) fn json_opt_f64(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    value_as_f64(val, key).map(Some)
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
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let raw = value_as_u64(val, key)?;
    u8::try_from(raw)
        .map(Some)
        .map_err(|_| field_err(key, "u8 out of range"))
}

pub(crate) fn json_opt_u32(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u32>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let raw = value_as_u64(val, key)?;
    u32::try_from(raw)
        .map(Some)
        .map_err(|_| field_err(key, "u32 out of range"))
}

pub(crate) fn json_opt_u64(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    value_as_u64(val, key).map(Some)
}

pub(crate) fn json_opt_usize(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<usize>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let raw = value_as_u64(val, key)?;
    usize::try_from(raw)
        .map(Some)
        .map_err(|_| field_err(key, "usize out of range"))
}

pub(crate) fn json_opt_string_vec(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let Some(items) = val.as_array() else {
        return Err(field_err(key, "expected array"));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(field_err(key, "expected string element"));
        };
        out.push(text.to_owned());
    }
    Ok(Some(out))
}

pub(crate) fn json_string_vec_or_empty(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, serde_json::Error> {
    Ok(json_opt_string_vec(map, key)?.unwrap_or_default())
}

pub(crate) fn json_opt_value_map(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<HashMap<String, Value>>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let Some(obj) = val.as_object() else {
        return Err(field_err(key, "expected object"));
    };
    Ok(Some(
        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ))
}

pub(crate) fn json_value_map_or_empty(
    map: &Map<String, Value>,
    key: &str,
) -> Result<HashMap<String, Value>, serde_json::Error> {
    Ok(json_opt_value_map(map, key)?.unwrap_or_default())
}

pub(crate) fn json_f64_pair(value: &Value, field: &str) -> Result<[f64; 2], serde_json::Error> {
    let Some(items) = value.as_array() else {
        return Err(field_err(field, "expected [f64; 2]"));
    };
    let [a, b] = items.as_slice() else {
        return Err(field_err(field, "expected 2 numbers"));
    };
    Ok([value_as_f64(a, field)?, value_as_f64(b, field)?])
}

pub(crate) fn json_opt_f64_pair(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<[f64; 2]>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    json_f64_pair(val, key).map(Some)
}

pub(crate) fn json_opt_bounds4(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<(f64, f64, f64, f64)>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let Some(items) = val.as_array() else {
        return Err(field_err(key, "expected bounds array"));
    };
    let [a, b, c, d] = items.as_slice() else {
        return Err(field_err(key, "expected 4 numbers"));
    };
    Ok(Some((
        value_as_f64(a, key)?,
        value_as_f64(b, key)?,
        value_as_f64(c, key)?,
        value_as_f64(d, key)?,
    )))
}

pub(crate) fn json_req_coords4(
    map: &Map<String, Value>,
    key: &str,
) -> Result<[[f64; 2]; 4], serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Err(missing_field_err(key));
    };
    let Some(items) = val.as_array() else {
        return Err(field_err(key, "expected 4 coordinates"));
    };
    let [p0, p1, p2, p3] = items.as_slice() else {
        return Err(field_err(key, "expected 4 coordinates"));
    };
    Ok([
        json_f64_pair(p0, key)?,
        json_f64_pair(p1, key)?,
        json_f64_pair(p2, key)?,
        json_f64_pair(p3, key)?,
    ])
}

pub(crate) fn json_req_coords(
    map: &Map<String, Value>,
    key: &str,
) -> Result<crate::coords::WorldTileCoords, serde_json::Error> {
    let coords_map = map
        .get(key)
        .and_then(|v| v.as_object())
        .ok_or_else(|| missing_field_err(key))?;
    let x = coords_map
        .get("x")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| field_err("coords.x", "expected i32"))?;
    let y = coords_map
        .get("y")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| field_err("coords.y", "expected i32"))?;
    let z = json_opt_u8(coords_map, "z")?.ok_or_else(|| missing_field_err("coords.z"))?;
    Ok(crate::coords::WorldTileCoords {
        x,
        y,
        z: crate::coords::ZoomLevel::new(z),
    })
}

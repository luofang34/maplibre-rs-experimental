//! Binary wire codec for feature property maps and typed expression values.

use rustc_hash::FxHashMap;

use crate::style::expression::{Color, FeatureProperties, Value};

const TAG_NULL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_NUMBER: u8 = 3;
const TAG_STRING: u8 = 4;
const TAG_COLOR: u8 = 5;
const TAG_ARRAY: u8 = 6;
const TAG_OBJECT: u8 = 7;

fn encode_u32_le(value: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn encode_color_value(color: &Color, out: &mut Vec<u8>) {
    out.push(TAG_COLOR);
    for ch in [color.r, color.g, color.b, color.a] {
        out.extend_from_slice(&ch.to_le_bytes());
    }
}

fn encode_array_value(items: &[Value], out: &mut Vec<u8>) {
    out.push(TAG_ARRAY);
    encode_u32_le(items.len() as u32, out);
    for item in items {
        encode_value(item, out);
    }
}

fn encode_object_value(map: &FxHashMap<String, Value>, out: &mut Vec<u8>) {
    out.push(TAG_OBJECT);
    encode_u32_le(map.len() as u32, out);
    for (k, v) in map {
        encode_str(k, out);
        encode_value(v, out);
    }
}

pub(crate) fn encode_properties(props: &FeatureProperties, out: &mut Vec<u8>) {
    encode_u32_le(props.len() as u32, out);
    for (key, value) in props {
        encode_str(key, out);
        encode_value(value, out);
    }
}

pub(crate) fn encode_str(s: &str, out: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    encode_u32_le(bytes.len() as u32, out);
    out.extend_from_slice(bytes);
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(false) => out.push(TAG_FALSE),
        Value::Bool(true) => out.push(TAG_TRUE),
        Value::Number(n) => {
            out.push(TAG_NUMBER);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::String(s) => {
            out.push(TAG_STRING);
            encode_str(s, out);
        }
        Value::Color(c) => encode_color_value(c, out),
        Value::Array(items) => encode_array_value(items, out),
        Value::Object(map) => encode_object_value(map, out),
        Value::Collator(_)
        | Value::Image(_)
        | Value::Formatted(_)
        | Value::Padding(_)
        | Value::NumberArray(_)
        | Value::ColorArray(_)
        | Value::ProjectionDefinition(_)
        | Value::VariableAnchorOffsetCollection(_) => {
            out.push(TAG_NULL);
        }
    }
}

pub(super) fn decode_properties(mut buf: &[u8]) -> Option<FeatureProperties> {
    decode_properties_from(&mut buf)
}

pub(crate) fn decode_properties_from(buf: &mut &[u8]) -> Option<FeatureProperties> {
    let count = read_u32(buf)? as usize;
    let mut props = FeatureProperties::with_capacity_and_hasher(count.min(256), Default::default());
    for _ in 0..count {
        let key = read_string(buf)?;
        let val = decode_value(buf)?;
        props.insert(key, val);
    }
    Some(props)
}

pub(crate) fn read_u8(buf: &mut &[u8]) -> Option<u8> {
    let (&byte, rest) = buf.split_first()?;
    *buf = rest;
    Some(byte)
}

fn take_fixed_bytes<const N: usize>(buf: &mut &[u8]) -> Option<[u8; N]> {
    if buf.len() < N {
        return None;
    }
    let (head, rest) = buf.split_at(N);
    *buf = rest;
    head.try_into().ok()
}

pub(crate) fn read_u32(buf: &mut &[u8]) -> Option<u32> {
    take_fixed_bytes::<4>(buf).map(u32::from_le_bytes)
}

pub(crate) fn read_f32(buf: &mut &[u8]) -> Option<f32> {
    take_fixed_bytes::<4>(buf).map(f32::from_le_bytes)
}

pub(crate) fn read_f64(buf: &mut &[u8]) -> Option<f64> {
    take_fixed_bytes::<8>(buf).map(f64::from_le_bytes)
}

pub(crate) fn read_u64(buf: &mut &[u8]) -> Option<u64> {
    take_fixed_bytes::<8>(buf).map(u64::from_le_bytes)
}

pub(crate) fn read_string(buf: &mut &[u8]) -> Option<String> {
    let len = read_u32(buf)? as usize;
    if buf.len() < len {
        return None;
    }
    let (head, rest) = buf.split_at(len);
    *buf = rest;
    std::str::from_utf8(head).ok().map(str::to_owned)
}

fn decode_color_value(buf: &mut &[u8]) -> Option<Color> {
    Some(Color::new(
        read_f64(buf)?,
        read_f64(buf)?,
        read_f64(buf)?,
        read_f64(buf)?,
    ))
}

fn decode_array_value(buf: &mut &[u8]) -> Option<Vec<Value>> {
    let len = read_u32(buf)? as usize;
    let mut items = Vec::with_capacity(len.min(1024));
    for _ in 0..len {
        items.push(decode_value(buf)?);
    }
    Some(items)
}

fn decode_object_value(buf: &mut &[u8]) -> Option<FxHashMap<String, Value>> {
    let len = read_u32(buf)? as usize;
    let mut map = FxHashMap::default();
    for _ in 0..len {
        let k = read_string(buf)?;
        let v = decode_value(buf)?;
        map.insert(k, v);
    }
    Some(map)
}

fn decode_value(buf: &mut &[u8]) -> Option<Value> {
    match read_u8(buf)? {
        TAG_NULL => Some(Value::Null),
        TAG_FALSE => Some(Value::Bool(false)),
        TAG_TRUE => Some(Value::Bool(true)),
        TAG_NUMBER => Some(Value::Number(read_f64(buf)?)),
        TAG_STRING => Some(Value::String(read_string(buf)?)),
        TAG_COLOR => decode_color_value(buf).map(Value::Color),
        TAG_ARRAY => decode_array_value(buf).map(Value::Array),
        TAG_OBJECT => decode_object_value(buf).map(Value::Object),
        _ => None,
    }
}

//! Compact JSON serializer helpers avoiding `serde_json::ser` / `zmij` monomorphization.

use std::io::Write;

use serde_json::Value;

/// Formats a number the way JavaScript's `String(number)` does.
pub fn js_number(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    if number == 0.0 {
        return "0".to_string();
    }
    if (1e-6..1e21).contains(&number.abs()) {
        return format!("{number}");
    }
    format_scientific_js(number)
}

fn format_scientific_js(number: f64) -> String {
    let scientific = format!("{number:e}");
    match scientific.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => scientific,
    }
}

fn write_control_escape(byte: u8, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hi = HEX.get((byte >> 4) as usize).copied().unwrap_or(b'0');
    let lo = HEX.get((byte & 0xf) as usize).copied().unwrap_or(b'0');
    out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hi, lo]);
}

fn named_json_escape(byte: u8) -> Option<&'static [u8]> {
    match byte {
        b'"' => Some(b"\\\""),
        b'\\' => Some(b"\\\\"),
        b'\x08' => Some(b"\\b"),
        b'\x0c' => Some(b"\\f"),
        b'\n' => Some(b"\\n"),
        b'\r' => Some(b"\\r"),
        b'\t' => Some(b"\\t"),
        _ => None,
    }
}

fn write_json_str(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let bytes = text.as_bytes();
    let mut start = 0;
    for (idx, byte) in text.bytes().enumerate() {
        if let Some(esc) = named_json_escape(byte) {
            if let Some(prefix) = bytes.get(start..idx) {
                out.extend_from_slice(prefix);
            }
            out.extend_from_slice(esc);
            start = idx + 1;
        } else if byte <= 0x1f {
            if let Some(prefix) = bytes.get(start..idx) {
                out.extend_from_slice(prefix);
            }
            write_control_escape(byte, out);
            start = idx + 1;
        }
    }
    if let Some(tail) = bytes.get(start..) {
        out.extend_from_slice(tail);
    }
    out.push(b'"');
}

fn write_json_number(n: &serde_json::Number, out: &mut Vec<u8>) {
    if let Some(i) = n.as_i64() {
        drop(write!(out, "{i}"));
    } else if let Some(u) = n.as_u64() {
        drop(write!(out, "{u}"));
    } else if let Some(f) = n.as_f64()
        && f.is_finite()
    {
        let s = js_number(f);
        out.extend_from_slice(s.as_bytes());
    } else {
        out.extend_from_slice(b"null");
    }
}

fn write_json_array(items: &[Value], out: &mut Vec<u8>) {
    out.push(b'[');
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push(b',');
        }
        write_json_bytes(item, out);
    }
    out.push(b']');
}

fn write_json_object(entries: &serde_json::Map<String, Value>, out: &mut Vec<u8>) {
    out.push(b'{');
    for (idx, (k, v)) in entries.iter().enumerate() {
        if idx > 0 {
            out.push(b',');
        }
        write_json_str(k, out);
        out.push(b':');
        write_json_bytes(v, out);
    }
    out.push(b'}');
}

/// Encodes a [`Value`] as compact UTF-8 JSON without pulling `serde_json::ser`/`zmij`.
pub(crate) fn write_json_bytes(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_json_number(n, out),
        Value::String(text) => write_json_str(text, out),
        Value::Array(items) => write_json_array(items, out),
        Value::Object(entries) => write_json_object(entries, out),
    }
}

/// Encodes a [`Value`] as compact JSON string without pulling `serde_json::ser`/`zmij`.
pub(crate) fn json_value_to_string(value: &Value) -> String {
    let mut bytes = Vec::with_capacity(64);
    write_json_bytes(value, &mut bytes);
    String::from_utf8(bytes).unwrap_or_default()
}

/// Encodes string items as a compact JSON array without pulling `serde_json::ser`/`zmij`.
pub(crate) fn encode_json_str_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = Vec::with_capacity(32);
    out.push(b'[');
    for (idx, item) in items.into_iter().enumerate() {
        if idx > 0 {
            out.push(b',');
        }
        write_json_str(item, &mut out);
    }
    out.push(b']');
    String::from_utf8(out).unwrap_or_default()
}

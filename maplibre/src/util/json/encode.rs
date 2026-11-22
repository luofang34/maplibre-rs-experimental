//! Compact JSON serializer helpers avoiding `serde_json::ser` / `zmij` monomorphization.

use std::io::Write;

use serde_json::Value;

use crate::style::expression::js_number;

fn write_json_str(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let bytes = text.as_bytes();
    let mut start = 0;
    for (idx, byte) in text.bytes().enumerate() {
        let esc: &[u8] = match byte {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\x08' => b"\\b",
            b'\x0c' => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0..=0x1f => {
                if let Some(prefix) = bytes.get(start..idx) {
                    out.extend_from_slice(prefix);
                }
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let hi = HEX.get((byte >> 4) as usize).copied().unwrap_or(b'0');
                let lo = HEX.get((byte & 0xf) as usize).copied().unwrap_or(b'0');
                out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hi, lo]);
                start = idx + 1;
                continue;
            }
            _ => continue,
        };
        if let Some(prefix) = bytes.get(start..idx) {
            out.extend_from_slice(prefix);
        }
        out.extend_from_slice(esc);
        start = idx + 1;
    }
    if let Some(tail) = bytes.get(start..) {
        out.extend_from_slice(tail);
    }
    out.push(b'"');
}

/// Encodes a [`Value`] as compact UTF-8 JSON without pulling `serde_json::ser`/`zmij`.
pub(crate) fn write_json_bytes(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                drop(write!(out, "{i}"));
            } else if let Some(u) = n.as_u64() {
                drop(write!(out, "{u}"));
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() {
                    let s = js_number(f);
                    out.extend_from_slice(s.as_bytes());
                } else {
                    out.extend_from_slice(b"null");
                }
            } else {
                out.extend_from_slice(b"null");
            }
        }
        Value::String(text) => write_json_str(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(b',');
                }
                write_json_bytes(item, out);
            }
            out.push(b']');
        }
        Value::Object(entries) => {
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

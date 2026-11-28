//! Shared JSON deserializers avoiding per-type `StrRead`/`SliceRead` and struct `visit_seq` bloat.

mod encode;
pub(crate) mod extract;
#[cfg(any(test, not(target_arch = "wasm32")))]
mod value_ref;

pub use encode::js_number;
pub(crate) use encode::{encode_json_str_list, json_value_to_string, write_json_bytes};
use serde_json::{Map, Value};
#[cfg(any(test, not(target_arch = "wasm32")))]
pub use value_ref::{from_json_slice, from_json_str, from_json_value};

/// Inserts a key-value tag into an object map when serializing internally tagged enums.
pub(crate) fn serialize_tagged_object<S: serde::Serializer, T: serde::Serialize>(
    tag: &'static str,
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let encoded = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    let mut map = match encoded {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    map.insert("type".to_owned(), Value::String(tag.to_owned()));
    serde::Serialize::serialize(&map, serializer)
}

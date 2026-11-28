//! A feature's id and geometry type, for `["id"]` and `["geometry-type"]` outside filters.
//!
//! A filter sees both in its own context. Paint and layout are evaluated where only a feature's
//! properties travel, so a layer whose paint or layout reads either gets them as properties of
//! their own, as a feature's geometry travels for `within` and `distance`.

use serde_json::Value as Json;

/// The property that carries a feature's id.
pub const ID_PROPERTY: &str = "\u{1}id";

/// The property that carries a feature's geometry type, as `["geometry-type"]` names it.
pub const GEOMETRY_TYPE_PROPERTY: &str = "\u{1}geometry-type";

/// Whether a property is one the renderer adds to a feature rather than one its source gave.
pub fn is_internal_property(key: &str) -> bool {
    key.starts_with('\u{1}')
}

/// Whether `value`, such as a layer's paint and layout, has an expression that reads the
/// feature's id or geometry type.
pub fn reads_identity(value: &Json) -> bool {
    match value {
        Json::Array(items) => {
            matches!(
                items.as_slice(),
                [Json::String(operator)] if operator == "id" || operator == "geometry-type"
            ) || items.iter().any(reads_identity)
        }
        Json::Object(fields) => fields.values().any(reads_identity),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

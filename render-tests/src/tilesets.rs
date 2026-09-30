//! The TileJSON documents of the fixtures' local tile sets, read into the sources that name them.

use std::path::PathBuf;

use serde_json::Value;

/// The file behind a `local://tilesets/...` TileJSON URL.
fn local_tileset_path(url: &str) -> Option<PathBuf> {
    let relative = url.strip_prefix("local://tilesets/")?;
    Some(PathBuf::from("render-tests/src/assets/tilesets").join(relative))
}

/// Fills in a source that names a local TileJSON by its URL: the document's tiles and limits
/// stand where the source does not declare its own, as GL JS lets a style overwrite them.
pub(super) fn resolve_source(source: &mut Value) -> Result<(), String> {
    let Some(object) = source.as_object_mut() else {
        return Ok(());
    };
    let Some(path) = object
        .get("url")
        .and_then(Value::as_str)
        .and_then(local_tileset_path)
    else {
        return Ok(());
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Cannot parse {}: {error}", path.display()))?;
    if let Some(members) = document.as_object() {
        for (key, value) in members {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    object.remove("url");
    Ok(())
}

/// Resolves every source of a style document.
pub(super) fn resolve_sources(style: &mut Value) -> Result<(), String> {
    let Some(sources) = style.get_mut("sources").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    sources.values_mut().try_for_each(resolve_source)
}

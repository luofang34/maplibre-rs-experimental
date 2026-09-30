//! `text-translate` and `icon-translate`: a fixed shift of a symbol's anchor.

use crate::style::layer::SymbolPaint;

/// Tile units in one pixel of a tile drawn at its own zoom: 4096 units span 512 pixels.
const TILE_UNITS_PER_PIXEL: f64 = 8.0;

fn translate(paint: &SymbolPaint, prefix: &str) -> [f64; 2] {
    let read = |index: usize| {
        paint
            .properties
            .get(&format!("{prefix}-translate"))
            .and_then(|value| value.get(index))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
    };
    [read(0), read(1)]
}

/// Whether the translation follows the viewport axes instead of the map's.
pub(crate) fn viewport_translation(paint: &SymbolPaint, prefix: &str) -> bool {
    translate(paint, prefix) != [0.0; 2]
        && paint
            .properties
            .get(&format!("{prefix}-translate-anchor"))
            .and_then(serde_json::Value::as_str)
            == Some("viewport")
}

/// The shift of the anchor in tile units; a translation in viewport axes is not applied.
pub(crate) fn tile_translation(paint: &SymbolPaint, prefix: &str) -> [f64; 2] {
    if viewport_translation(paint, prefix) {
        return [0.0; 2];
    }
    translate(paint, prefix).map(|pixels| pixels * TILE_UNITS_PER_PIXEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(json: serde_json::Value) -> SymbolPaint {
        serde_json::from_value(json).expect("valid symbol paint")
    }

    #[test]
    fn map_translation_is_in_tile_units_and_viewport_translation_is_left_out() {
        let map = paint(serde_json::json!({"text-translate": [2, -1]}));
        assert_eq!(tile_translation(&map, "text"), [16.0, -8.0]);
        assert_eq!(tile_translation(&map, "icon"), [0.0, 0.0]);
        let viewport = paint(serde_json::json!({
            "icon-translate": [2, 0], "icon-translate-anchor": "viewport"
        }));
        assert!(viewport_translation(&viewport, "icon"));
        assert_eq!(tile_translation(&viewport, "icon"), [0.0, 0.0]);
    }
}

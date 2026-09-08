use super::*;
#[test]
fn zoom_paint_uses_one_view_zoom_for_all_source_levels() -> Result<(), serde_json::Error> {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "woods", "type": "fill", "paint": {
            "fill-color": ["interpolate", ["linear"], ["zoom"], 4, "#000000", 12, "#ffffff"],
            "fill-opacity": ["interpolate", ["linear"], ["zoom"], 4, 0, 12, 1]
        }
    }))?;
    assert_eq!(uniform_color(&layer, 8.0), Some([0.5, 0.5, 0.5, 0.5]));
    assert_eq!(uniform_color(&layer, 12.0), Some([1.0; 4]));
    Ok(())
}

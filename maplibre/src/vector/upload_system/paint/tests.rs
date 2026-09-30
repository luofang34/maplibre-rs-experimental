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

#[test]
fn stationary_view_jitter_does_not_alternate_drape_styles() {
    let mut previous = None;
    for requested in [12.059, 12.066, 12.061, 12.071, 12.062] {
        let selected = super::next_zoom(previous, requested);
        assert_eq!(selected, 12.0);
        previous = Some(selected);
    }
    assert_eq!(super::next_zoom(previous, 12.3), 12.25);
    assert_eq!(super::next_zoom(Some(12.25), 12.13), 12.25);
}

#[test]
fn opacity_beyond_the_unit_range_is_clamped_in_the_layer_colour() {
    for (opacity, alpha) in [(2.0, 1.0), (-1.0, 0.0), (0.25, 0.25)] {
        let layer: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "land", "type": "fill", "source": "s", "source-layer": "land",
            "paint": {"fill-color": "#0000ff", "fill-opacity": opacity}
        }))
        .expect("layer");
        let color = uniform_color(&layer, 5.0).expect("uniform colour");
        assert!(
            (color[3] - alpha).abs() < 1e-6,
            "{opacity} gave {}",
            color[3]
        );
    }
}

use csscolorparser::Color;
use serde_json::json;

use super::{interpolate_paint_value, TransitionSpec};
use crate::style::{
    layer::{LayerPaint, StyleProperty},
    Style,
};

fn background_rgb8(style: &Style) -> [u8; 4] {
    let Some(LayerPaint::Background(bg)) = &style.layers[0].paint else {
        panic!("expected background layer");
    };
    let Some(StyleProperty::Constant(color)) = &bg.background_color else {
        panic!("expected constant background color");
    };
    color.to_rgba8()
}

#[test]
fn freshly_loaded_style_does_not_animate_initial_paint_values() {
    let style: Style = serde_json::from_value(json!({
        "version": 8,
        "transition": { "duration": 1000, "delay": 200 },
        "sources": {},
        "layers": [{
            "id": "bg",
            "type": "background",
            "paint": {
                "background-color": "#ff0000",
                "background-color-transition": { "duration": 500, "delay": 100 }
            }
        }]
    }))
    .expect("style parses");

    assert!(!style.has_active_transitions());
    assert_eq!(
        style.transition,
        Some(TransitionSpec {
            duration: 1000.0,
            delay: 200.0
        })
    );
    assert_eq!(
        style.layers[0].transitions.get("background-color"),
        Some(&TransitionSpec {
            duration: 500.0,
            delay: 100.0
        })
    );
    assert_eq!(background_rgb8(&style), [255, 0, 0, 255]);
    assert!(
        style.validate().is_ok(),
        "*-transition declarations must validate cleanly"
    );

    let round_trip: Style =
        serde_json::from_value(serde_json::to_value(&style).expect("serialize"))
            .expect("deserialize");
    assert_eq!(
        round_trip.layers[0].transitions.get("background-color"),
        Some(&TransitionSpec {
            duration: 500.0,
            delay: 100.0
        })
    );
}

#[test]
fn layer_and_root_transitions_interpolate_colors_numbers_translations_and_arrays() {
    let mut style: Style = serde_json::from_value(json!({
        "version": 8,
        "transition": { "duration": 400, "delay": 100 },
        "sources": {
            "pts": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": {
                    "background-color": "rgba(255, 0, 0, 1)",
                    "background-opacity": 0.2,
                    "background-color-transition": { "duration": 1000, "delay": 0 }
                }
            },
            {
                "id": "fill",
                "type": "fill",
                "source": "pts",
                "paint": {
                    "fill-translate": [-10.0, 20.0]
                }
            }
        ]
    }))
    .expect("style parses");

    // Mutate background-color (uses layer 1000ms, 0ms delay), background-opacity (uses root 400ms, 100ms delay),
    // and fill-translate (uses root 400ms, 100ms delay).
    style
        .set_paint_property("bg", "background-color", json!("rgba(0, 0, 255, 1)"))
        .expect("set bg color");
    style
        .set_paint_property("bg", "background-opacity", json!(1.0))
        .expect("set bg opacity");
    style
        .set_paint_property("fill", "fill-translate", json!([10.0, -20.0]))
        .expect("set fill translate");

    assert!(style.has_active_transitions());
    assert_eq!(background_rgb8(&style), [255, 0, 0, 255]);

    // Advance 100ms: bg-color at 10%, bg-opacity & fill-translate still at end of 100ms delay (0%).
    let step1 = style.advance_transitions(100.0);
    assert!(step1.layers.contains(&"bg".to_owned()));
    let rgba_100 = background_rgb8(&style);
    assert!((i16::from(rgba_100[0]) - 230).abs() <= 1);
    assert!((i16::from(rgba_100[2]) - 26).abs() <= 1);

    // Advance another 200ms (clock = 300ms):
    // - bg-color at 300/1000 = 30%
    // - bg-opacity at (300 - 100)/400 = 50% -> 0.6
    // - fill-translate at 50% -> [0.0, 0.0]
    style.advance_transitions(200.0);
    let Some(LayerPaint::Background(bg)) = &style.layers[0].paint else {
        panic!("bg");
    };
    let opacity = bg
        .background_opacity
        .as_ref()
        .and_then(|p| p.evaluate_at_zoom(0.0))
        .expect("opacity");
    assert!((opacity - 0.6).abs() < 1e-4, "expected 0.6, got {opacity}");

    let Some(LayerPaint::Fill(fill)) = &style.layers[1].paint else {
        panic!("fill");
    };
    assert_eq!(
        fill.fill_translate.as_ref().map(|t| t.at_zoom(0.0)),
        Some([0.0, 0.0])
    );

    // Advance past 1000ms: all transitions settle and clear.
    style.advance_transitions(800.0);
    assert!(!style.has_active_transitions());
    assert_eq!(background_rgb8(&style), [0, 0, 255, 255]);

    // Verify premultiplied alpha color interpolation and fixed-length array interpolation directly.
    let mid_pm = interpolate_paint_value(
        &json!("rgba(255, 0, 0, 1)"),
        &json!("rgba(0, 0, 255, 0)"),
        0.5,
    )
    .expect("premultiplied color");
    let parsed: Color = mid_pm.as_str().expect("css").parse().expect("color");
    let [r, g, b, a] = parsed.to_rgba8();
    assert_eq!([r, g, b], [255, 0, 0]);
    assert!((i16::from(a) - 128).abs() <= 1);

    let nums = interpolate_paint_value(&json!([1.0, 2.0, 3.0]), &json!([5.0, 6.0, 7.0]), 0.25)
        .expect("numeric array");
    assert_eq!(nums, json!([2.0, 3.0, 4.0]));

    let colors = interpolate_paint_value(
        &json!(["#000000", "#ffffff"]),
        &json!(["#ffffff", "#000000"]),
        0.5,
    )
    .expect("color array");
    assert_eq!(colors.as_array().map(Vec::len), Some(2));
}

#[test]
fn runtime_set_paint_property_accepts_transition_specs() {
    let mut style = Style::default();
    style
        .set_paint_property(
            "background",
            "background-color-transition",
            json!({ "duration": 500, "delay": 50 }),
        )
        .expect("set transition spec");
    style
        .set_paint_property("background", "background-color", json!("#0000ff"))
        .expect("start transition");
    assert!(style.has_active_transitions());
    style.settle_transitions_at(300.0);
    assert_eq!(background_rgb8(&style), [128, 128, 255, 255]);
    style.settle_transitions_at(550.0);
    assert!(!style.has_active_transitions());
    assert_eq!(background_rgb8(&style), [0, 0, 255, 255]);
}

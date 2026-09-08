#![allow(clippy::expect_used, clippy::panic)]
use maplibre::style::{
    expression::{FeatureProperties, Value},
    filter::{FeatureContext, Filter, GeometryType},
    layer::StyleLayer,
    Style,
};

fn style() -> Style {
    serde_json::from_str(include_str!(
        "../../apple/visionos/MapLibreVision/MapLibreVision/style.json"
    ))
    .expect("application style")
}

fn roads<'a>(style: &'a Style, class: &str, brunnel: &str, level: f64) -> Vec<&'a StyleLayer> {
    let properties: FeatureProperties = [
        ("class".into(), Value::String(class.into())),
        ("brunnel".into(), Value::String(brunnel.into())),
        ("layer".into(), Value::Number(level)),
    ]
    .into();
    let context = FeatureContext {
        properties: &properties,
        geometry_type: GeometryType::LineString,
        id: None,
        zoom: 18.0,
    };
    style
        .layers
        .iter()
        .filter(|layer| {
            layer.type_ == "line"
                && layer.source_layer.as_deref() == Some("transportation")
                && layer.is_visible_at(18.0)
                && layer
                    .filter
                    .as_ref()
                    .is_none_or(|filter| Filter::parse(filter).expect("filter").evaluate(&context))
        })
        .collect()
}

#[test]
fn road_and_rail_bridges_tunnels_and_surfaces_have_exclusive_working_filters() {
    let style = style();
    for class in ["primary", "motorway", "rail", "transit"] {
        for brunnel in ["bridge", "tunnel", ""] {
            let layers = roads(&style, class, brunnel, 1.0);
            assert!(!layers.is_empty(), "{class} {brunnel} disappears");
            for layer in &layers {
                assert_eq!(layer.id.starts_with("bridge_"), brunnel == "bridge");
                assert_eq!(layer.id.starts_with("tunnel_"), brunnel == "tunnel");
            }
            if brunnel != "tunnel" {
                assert_eq!(layers.len(), 2, "casing and inner for {class} {brunnel}");
            }
        }
    }
}

#[test]
fn higher_bridge_layers_paint_after_both_lower_bridge_strokes() {
    let style = style();
    let lower = roads(&style, "primary", "bridge", 1.0);
    let upper = roads(&style, "primary", "bridge", 2.0);
    assert!(
        lower.iter().map(|l| l.index).max().expect("lower bridge")
            < upper.iter().map(|l| l.index).min().expect("upper bridge")
    );
}

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

#[test]
fn surface_palette_remains_constant_across_source_zoom_levels() {
    use maplibre::style::layer::LayerPaint;
    let style = style();
    for id in [
        "landcover_wood",
        "landcover_glacier",
        "landcover_ice_shelf",
        "landuse_residential",
    ] {
        let layer = style
            .layers
            .iter()
            .find(|layer| layer.id == id)
            .expect("surface layer");
        let Some(LayerPaint::Fill(paint)) = &layer.paint else {
            panic!("fill required")
        };
        let color = paint.fill_color.as_ref().expect("surface color");
        let opacity = paint.fill_opacity.as_ref().expect("surface opacity");
        for zoom in [0.0, 4.0, 8.0, 12.0, 16.0, 20.0] {
            assert!(layer.is_visible_at(zoom), "{id} disappears at {zoom}");
            assert_eq!(color.evaluate_at_zoom(zoom), color.evaluate_at_zoom(8.0));
            assert_eq!(
                opacity.evaluate_at_zoom(zoom),
                opacity.evaluate_at_zoom(8.0)
            );
        }
    }
}

#[test]
fn place_detail_enters_progressively_and_keeps_major_places_larger() {
    use maplibre::style::layer::LayerPaint;
    let style = style();
    let mut previous_zoom = 0.0;
    let mut previous_size = f32::MAX;
    for id in [
        "place_city_large",
        "place_city",
        "place_town",
        "place_village",
        "place_suburb",
    ] {
        let layer = style
            .layers
            .iter()
            .find(|layer| layer.id == id)
            .expect("place layer");
        let zoom = layer.minzoom.expect("entry zoom") as f64;
        assert!(
            zoom > previous_zoom,
            "{id} must enter later than larger places"
        );
        assert!(!layer.is_visible_at(zoom - 0.01));
        assert!(layer.is_visible_at(zoom));
        let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
            panic!("symbol required")
        };
        let size = paint
            .text_size
            .as_ref()
            .expect("size")
            .evaluate_at_zoom(zoom)
            .expect("value");
        assert!(
            size < previous_size,
            "{id} must be smaller than larger places"
        );
        previous_zoom = zoom;
        previous_size = size;
    }
}

#[test]
fn labels_grow_continuously_and_keep_halos_subordinate_to_type() {
    use maplibre::style::layer::LayerPaint;
    let style = style();
    for layer in style.layers.iter().filter(|l| l.id.starts_with("place_")) {
        let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
            panic!("place symbol")
        };
        let start = f64::from(layer.minzoom.unwrap_or(0));
        let end = f64::from(layer.maxzoom.unwrap_or(24)) - 0.1;
        let size = |z| {
            paint
                .text_size
                .as_ref()
                .expect("size")
                .evaluate_at_zoom(z)
                .expect("evaluated size")
        };
        assert!(size(end) > size(start), "{} lacks zoom hierarchy", layer.id);
        for step in 0..100 {
            let zoom = start + (end - start) * f64::from(step) / 100.0;
            assert!(
                (size(zoom + 0.001) - size(zoom)).abs() < 0.02,
                "size jumped"
            );
            let halo = paint.number("text-halo-width", &FeatureProperties::new(), zoom, 0.0);
            assert!(
                halo > 0.0 && halo < size(zoom) / 10.0,
                "heavy halo hides letter shape"
            );
        }
    }
}

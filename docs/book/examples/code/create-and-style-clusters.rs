use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        circle::CirclePaint,
        layer::{LayerPaint, StyleLayer, StyleProperty, SymbolPaint},
        source::{fresh_generation, GeoJsonData, GeoJsonSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
use serde_json::json;

const BASE_STYLE_URL: &str = "https://demotiles.maplibre.org/style.json";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([-103.59179687498357, 40.66995747013945]);
    style.zoom = Some(3.0);

    style.sources.insert(
        "earthquakes".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Url(
                "https://maplibre.org/maplibre-gl-js/docs/assets/earthquakes.geojson".to_string(),
            ),
            maxzoom: None,
            buffer: None,
            minzoom: None,
            promote_id: None,
            generate_id: false,
            cluster: true,
            cluster_radius: Some(50.0),
            cluster_max_zoom: Some(14),
            cluster_min_points: None,
            cluster_properties: None,
            filter: None,
            generation: fresh_generation(),
        }),
    );

    style.layers.push(StyleLayer {
        id: "clusters".to_string(),
        type_: "circle".to_string(),
        source: Some("earthquakes".to_string()),
        source_layer: None,
        filter: Some(json!(["has", "point_count"])),
        paint: Some(LayerPaint::Circle(CirclePaint {
            circle_color: Some(StyleProperty::parse(&json!([
                "step",
                ["get", "point_count"],
                "#51bbd6",
                100,
                "#f1f075",
                750,
                "#f28cb1"
            ]))),
            circle_radius: Some(StyleProperty::parse(&json!([
                "step",
                ["get", "point_count"],
                20,
                100,
                30,
                750,
                40
            ]))),
            ..Default::default()
        })),
        ..Default::default()
    });

    let mut cluster_count_paint = SymbolPaint::default();
    cluster_count_paint.text_field = Some(StyleProperty::parse(&json!("{point_count_abbreviated}")));
    cluster_count_paint.text_size = Some(StyleProperty::Constant(12.0));
    cluster_count_paint
        .properties
        .insert("text-font".to_string(), json!(["Noto Sans Regular"]));

    style.layers.push(StyleLayer {
        id: "cluster-count".to_string(),
        type_: "symbol".to_string(),
        source: Some("earthquakes".to_string()),
        source_layer: None,
        filter: Some(json!(["has", "point_count"])),
        paint: Some(LayerPaint::Symbol(cluster_count_paint)),
        ..Default::default()
    });

    style.layers.push(StyleLayer {
        id: "unclustered-point".to_string(),
        type_: "circle".to_string(),
        source: Some("earthquakes".to_string()),
        source_layer: None,
        filter: Some(json!(["!", ["has", "point_count"]])),
        paint: Some(LayerPaint::Circle(CirclePaint {
            circle_color: Some(StyleProperty::parse(&json!("#11b4da"))),
            circle_radius: Some(StyleProperty::Constant(4.0)),
            circle_stroke_width: Some(StyleProperty::Constant(1.0)),
            circle_stroke_color: Some(StyleProperty::parse(&json!("#fff"))),
            ..Default::default()
        })),
        ..Default::default()
    });

    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base: Style = maplibre::platform::run_multithreaded(async {
        reqwest::get(BASE_STYLE_URL).await?.json().await
    })?;
    let style = build_style(base);

    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Create and Style Clusters".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

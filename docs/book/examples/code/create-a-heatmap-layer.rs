use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        circle::CirclePaint,
        heatmap::HeatmapPaint,
        layer::{LayerPaint, StyleLayer, StyleProperty},
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
    style.center = Some([-120.0, 50.0]);
    style.zoom = Some(2.0);

    style.sources.insert(
        "earthquakes".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Url(
                "https://maplibre.org/maplibre-gl-js/docs/assets/earthquakes.geojson".to_string(),
            ),
            maxzoom: None,
            buffer: None,
            tolerance: None,
            line_metrics: false,
            minzoom: None,
            promote_id: None,
            generate_id: false,
            cluster: false,
            cluster_radius: None,
            cluster_max_zoom: None,
            cluster_min_points: None,
            cluster_properties: None,
            filter: None,
            volatile: false,
            generation: fresh_generation(),
        }),
    );

    style.layers.push(StyleLayer {
        id: "earthquakes-heat".to_string(),
        type_: "heatmap".to_string(),
        source: Some("earthquakes".to_string()),
        source_layer: None,
        maxzoom: Some(9.0),
        paint: Some(LayerPaint::Heatmap(HeatmapPaint {
            heatmap_weight: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["get", "mag"],
                0,
                0,
                6,
                1
            ]))),
            heatmap_intensity: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                0,
                1,
                9,
                3
            ]))),
            heatmap_color: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["heatmap-density"],
                0,
                "rgba(33,102,172,0)",
                0.2,
                "rgb(103,169,207)",
                0.4,
                "rgb(209,229,240)",
                0.6,
                "rgb(253,219,199)",
                0.8,
                "rgb(239,138,98)",
                1,
                "rgb(178,24,43)"
            ]))),
            heatmap_radius: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                0,
                2,
                9,
                20
            ]))),
            heatmap_opacity: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                7,
                1,
                9,
                0
            ]))),
        })),
        ..Default::default()
    });

    style.layers.push(StyleLayer {
        id: "earthquakes-point".to_string(),
        type_: "circle".to_string(),
        source: Some("earthquakes".to_string()),
        source_layer: None,
        minzoom: Some(7.0),
        paint: Some(LayerPaint::Circle(CirclePaint {
            circle_radius: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                7,
                ["interpolate", ["linear"], ["get", "mag"], 1, 1, 6, 4],
                16,
                ["interpolate", ["linear"], ["get", "mag"], 1, 5, 6, 50]
            ]))),
            circle_color: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["get", "mag"],
                1,
                "rgba(33,102,172,0)",
                2,
                "rgb(103,169,207)",
                3,
                "rgb(209,229,240)",
                4,
                "rgb(253,219,199)",
                5,
                "rgb(239,138,98)",
                6,
                "rgb(178,24,43)"
            ]))),
            circle_stroke_color: Some(StyleProperty::parse(&json!("white"))),
            circle_stroke_width: Some(StyleProperty::Constant(1.0)),
            circle_opacity: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                7,
                0,
                8,
                1
            ]))),
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
        WinitMapWindowConfig::new("Create a Heatmap Layer".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

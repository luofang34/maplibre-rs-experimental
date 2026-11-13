use std::{path::PathBuf, sync::Arc};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, LayerProperties, LinePaint, StyleLayer, StyleProperty},
        source::{fresh_generation, GeoJsonData, GeoJsonSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
use serde_json::{json, Map};

const BASE_STYLE_URL: &str = "https://tiles.openfreemap.org/styles/bright";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([-77.035, 38.875]);
    style.zoom = Some(12.0);

    style.sources.insert(
        "line".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Inline(Arc::new(json!({
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "properties": {},
                        "geometry": {
                            "type": "LineString",
                            "coordinates": [
                                [-77.044211, 38.852924],
                                [-77.045659, 38.860158],
                                [-77.044232, 38.862326],
                                [-77.040879, 38.865454],
                                [-77.039936, 38.867698],
                                [-77.040338, 38.86943],
                                [-77.04264, 38.872528],
                                [-77.03696, 38.878424],
                                [-77.032309, 38.87937],
                                [-77.030056, 38.880945],
                                [-77.027645, 38.881779],
                                [-77.026946, 38.882645],
                                [-77.026942, 38.885502],
                                [-77.028054, 38.887449],
                                [-77.02806, 38.892088],
                                [-77.03364, 38.892108],
                                [-77.033643, 38.899926]
                            ]
                        }
                    }
                ]
            }))),
            maxzoom: None,
            buffer: None,
            minzoom: None,
            promote_id: None,
            generate_id: false,
            cluster: false,
            cluster_radius: None,
            cluster_max_zoom: None,
            cluster_min_points: None,
            cluster_properties: None,
            filter: None,
            generation: fresh_generation(),
        }),
    );

    style.layers.push(StyleLayer {
        id: "line".to_string(),
        type_: "line".to_string(),
        source: Some("line".to_string()),
        source_layer: None,
        paint: Some(LayerPaint::Line(LinePaint {
            line_width: Some(StyleProperty::Constant(12.0)),
            line_gradient: Some(StyleProperty::parse(&json!([
                "interpolate",
                ["linear"],
                ["line-progress"],
                0.0,
                "blue",
                0.1,
                "royalblue",
                0.3,
                "cyan",
                0.5,
                "lime",
                0.7,
                "yellow",
                1.0,
                "red"
            ]))),
            ..Default::default()
        })),
        unrecognized: LayerProperties {
            paint: Map::new(),
            layout: Map::from_iter([
                ("line-join".to_string(), json!("round")),
                ("line-cap".to_string(), json!("round")),
            ]),
        },
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
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Create a Gradient Line".to_string()),
        WgpuSettings::default(),
        build_style(base),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

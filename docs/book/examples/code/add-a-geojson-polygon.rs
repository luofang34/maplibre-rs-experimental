use std::{path::PathBuf, sync::Arc};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{FillPaint, LayerPaint, StyleLayer, StyleProperty},
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
    style.center = Some([-68.13734351262877, 45.137451890638886]);
    style.zoom = Some(5.0);

    style.sources.insert(
        "maine".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Inline(Arc::new(json!({
                "type": "Feature",
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[
                        [-67.13734351262877, 45.137451890638886],
                        [-66.96466, 44.8097],
                        [-68.03252, 44.3252],
                        [-69.06, 43.98],
                        [-70.11617, 43.68405],
                        [-70.64573401557249, 43.090083319667144],
                        [-70.75102474636725, 43.08003225358635],
                        [-70.79761105007827, 43.21973948828747],
                        [-70.98176001655037, 43.36789581966826],
                        [-70.94416541205806, 43.46633942318431],
                        [-71.08482, 45.3052400000002],
                        [-70.6600225491012, 45.46022288673396],
                        [-70.30495378282376, 45.914794623389355],
                        [-70.00014034695016, 46.69317088478567],
                        [-69.23708614772835, 47.44777598732787],
                        [-68.90478084987546, 47.184794623394396],
                        [-68.23430497910454, 47.35462921812177],
                        [-67.79035274928509, 47.066248887716995],
                        [-67.79141211614706, 45.702585354182816],
                        [-67.13734351262877, 45.137451890638886]
                    ]]
                }
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
        id: "maine".to_string(),
        type_: "fill".to_string(),
        source: Some("maine".to_string()),
        source_layer: None,
        paint: Some(LayerPaint::Fill(FillPaint {
            fill_color: Some(StyleProperty::parse(&json!("#088"))),
            fill_opacity: Some(StyleProperty::Constant(0.8)),
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
        WinitMapWindowConfig::new("Add a GeoJSON Polygon".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

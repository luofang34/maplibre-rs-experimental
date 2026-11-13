use std::{collections::HashMap, path::PathBuf};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        hillshade::{ColorReliefPaint, HillshadeMethod, HillshadePaint},
        layer::{LayerPaint, StyleLayer, StyleProperty},
        source::{RasterDemSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
use serde_json::json;

pub fn style() -> Style {
    Style {
        version: 8,
        sources: HashMap::from([(
            "elevationSource".to_string(),
            Source::RasterDem(RasterDemSource {
                url: Some("https://demotiles.maplibre.org/terrain-tiles/tiles.json".to_string()),
                tile_size: Some(256),
                ..Default::default()
            }),
        )]),
        layers: vec![
            StyleLayer {
                index: 1,
                id: "color-relief".to_string(),
                type_: "color-relief".to_string(),
                source: Some("elevationSource".to_string()),
                paint: Some(LayerPaint::ColorRelief(ColorReliefPaint {
                    color_relief_opacity: Some(StyleProperty::Constant(0.85)),
                    color_relief_color: Some(StyleProperty::parse(&json!([
                        "interpolate",
                        ["linear"],
                        ["elevation"],
                        0,
                        "#2b5c8f",
                        500,
                        "#3b8c5a",
                        1000,
                        "#8fbc6b",
                        1500,
                        "#d9c589",
                        2000,
                        "#b88b5a",
                        2500,
                        "#8c6239",
                        3200,
                        "#ffffff"
                    ]))),
                })),
                ..Default::default()
            },
            StyleLayer {
                index: 2,
                id: "hillshade".to_string(),
                type_: "hillshade".to_string(),
                source: Some("elevationSource".to_string()),
                paint: Some(LayerPaint::Hillshade(HillshadePaint {
                    hillshade_method: HillshadeMethod::Standard,
                    hillshade_exaggeration: Some(StyleProperty::Constant(0.35)),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        center: Some([11.39085, 47.27574]),
        zoom: Some(10.5),
        pitch: Some(0.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Add a Color Relief Layer".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

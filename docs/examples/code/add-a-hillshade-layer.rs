use std::{collections::HashMap, path::PathBuf};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        hillshade::{HillshadeMethod, HillshadePaint},
        layer::{LayerPaint, StyleLayer, StyleProperty},
        property::NumberList,
        source::{RasterDemSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

pub fn style() -> Style {
    Style {
        version: 8,
        sources: HashMap::from([(
            "hillshadeSource".to_string(),
            Source::RasterDem(RasterDemSource {
                url: Some("https://demotiles.maplibre.org/terrain-tiles/tiles.json".to_string()),
                tile_size: Some(256),
                ..Default::default()
            }),
        )]),
        layers: vec![StyleLayer {
            index: 1,
            id: "hillshade".to_string(),
            type_: "hillshade".to_string(),
            source: Some("hillshadeSource".to_string()),
            paint: Some(LayerPaint::Hillshade(HillshadePaint {
                hillshade_method: HillshadeMethod::Standard,
                hillshade_illumination_direction: Some(StyleProperty::Constant(NumberList(vec![
                    315.0,
                ]))),
                hillshade_shadow_color: Some(StyleProperty::parse(&serde_json::json!("#000000"))),
                hillshade_highlight_color: Some(StyleProperty::parse(&serde_json::json!(
                    "#FFFFFF"
                ))),
                hillshade_accent_color: Some(StyleProperty::parse(&serde_json::json!(
                    "#000000"
                ))),
                hillshade_exaggeration: Some(StyleProperty::Constant(0.5)),
                ..Default::default()
            })),
            ..Default::default()
        }],
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        center: Some([11.491, 47.274]),
        zoom: Some(10.0),
        pitch: Some(0.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Add a Hillshade Layer".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

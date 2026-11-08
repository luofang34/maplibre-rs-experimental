use std::{collections::HashMap, path::PathBuf};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, RasterPaint, StyleLayer, StyleProperty},
        light::{LightAnchor, LightSpecification},
        sky::SkySpecification,
        source::{Source, VectorSource},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

pub fn style() -> Style {
    Style {
        version: 8,
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        sources: HashMap::from([(
            "satellite".to_string(),
            Source::Raster(VectorSource {
                attribution: None,
                bounds: None,
                maxzoom: None,
                minzoom: None,
                scheme: None,
                tiles: Some(vec![
                    "https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless-2020_3857/default/g/{z}/{y}/{x}.jpg"
                        .to_string(),
                ]),
                tile_size: None,
                url: None,
            }),
        )]),
        layers: vec![StyleLayer {
            index: 1,
            id: "Satellite".to_string(),
            type_: "raster".to_string(),
            source: Some("satellite".to_string()),
            paint: Some(LayerPaint::Raster(RasterPaint::default())),
            ..Default::default()
        }],
        sky: Some(SkySpecification {
            atmosphere_blend: Some(StyleProperty::parse(&serde_json::json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                0,
                1,
                5,
                1,
                7,
                0
            ]))),
            ..Default::default()
        }),
        light: Some(LightSpecification {
            anchor: StyleProperty::Constant(LightAnchor::Map),
            position: StyleProperty::Constant([1.5, 90.0, 80.0]),
            color: None,
            intensity: None,
        }),
        center: Some([137.9150899566626, 36.25956997955441]),
        zoom: Some(0.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Display a Globe with an Atmosphere".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

use std::{collections::HashMap, path::PathBuf};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, RasterPaint, StyleLayer},
        source::{Source, VectorSource},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

pub fn style() -> Style {
    Style {
        version: 8,
        sources: HashMap::from([(
            "satellite".to_string(),
            Source::Raster(VectorSource {
                tiles: Some(vec![
                    "https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless-2020_3857/default/g/{z}/{y}/{x}.jpg"
                        .to_string(),
                ]),
                tile_size: Some(256),
                ..Default::default()
            }),
        )]),
        layers: vec![StyleLayer {
            index: 1,
            id: "satellite".to_string(),
            type_: "raster".to_string(),
            source: Some("satellite".to_string()),
            paint: Some(LayerPaint::Raster(RasterPaint::default())),
            ..Default::default()
        }],
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        center: Some([137.9150899566626, 36.25956997955441]),
        zoom: Some(9.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Display a Satellite Map".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

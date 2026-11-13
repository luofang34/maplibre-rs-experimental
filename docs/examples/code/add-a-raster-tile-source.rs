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
            "raster-tiles".to_string(),
            Source::Raster(VectorSource {
                attribution: Some("© OpenStreetMap contributors".to_string()),
                bounds: None,
                maxzoom: Some(19),
                minzoom: Some(0),
                scheme: None,
                tiles: Some(vec![
                    "https://tile.openstreetmap.org/{z}/{x}/{y}.png".to_string()
                ]),
                tile_size: Some(256),
                url: None,
                promote_id: None,
            }),
        )]),
        layers: vec![StyleLayer {
            index: 1,
            id: "simple-tiles".to_string(),
            type_: "raster".to_string(),
            source: Some("raster-tiles".to_string()),
            paint: Some(LayerPaint::Raster(RasterPaint::default())),
            ..Default::default()
        }],
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        center: Some([0.0, 0.0]),
        zoom: Some(0.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Add a Raster Tile Source".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

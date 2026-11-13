use std::{collections::HashMap, path::PathBuf};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        hillshade::HillshadePaint,
        layer::{LayerPaint, LayerVisibility, RasterPaint, StyleLayer, StyleProperty},
        sky::SkySpecification,
        source::{RasterDemSource, Source, VectorSource},
        terrain::TerrainSpecification,
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

pub fn style() -> Style {
    Style {
        version: 8,
        sources: HashMap::from([
            (
                "osm".to_string(),
                Source::Raster(VectorSource {
                    attribution: Some("&copy; OpenStreetMap Contributors".to_string()),
                    bounds: None,
                    maxzoom: Some(19),
                    minzoom: None,
                    scheme: None,
                    tiles: Some(vec![
                        "https://a.tile.openstreetmap.org/{z}/{x}/{y}.png".to_string()
                    ]),
                    tile_size: Some(256),
                    url: None,
                    promote_id: None,
                }),
            ),
            (
                "terrainSource".to_string(),
                Source::RasterDem(RasterDemSource {
                    attribution: None,
                    bounds: None,
                    maxzoom: None,
                    minzoom: None,
                    tiles: None,
                    url: Some("https://tiles.mapterhorn.com/tilejson.json".to_string()),
                    tile_size: None,
                    encoding: None,
                    red_factor: 1.0,
                    green_factor: 1.0,
                    blue_factor: 1.0,
                    base_shift: 0.0,
                }),
            ),
            (
                "hillshadeSource".to_string(),
                Source::RasterDem(RasterDemSource {
                    attribution: None,
                    bounds: None,
                    maxzoom: None,
                    minzoom: None,
                    tiles: None,
                    url: Some("https://tiles.mapterhorn.com/tilejson.json".to_string()),
                    tile_size: None,
                    encoding: None,
                    red_factor: 1.0,
                    green_factor: 1.0,
                    blue_factor: 1.0,
                    base_shift: 0.0,
                }),
            ),
        ]),
        layers: vec![
            StyleLayer {
                index: 1,
                id: "osm".to_string(),
                type_: "raster".to_string(),
                source: Some("osm".to_string()),
                paint: Some(LayerPaint::Raster(RasterPaint::default())),
                ..Default::default()
            },
            StyleLayer {
                index: 2,
                id: "hills".to_string(),
                type_: "hillshade".to_string(),
                source: Some("hillshadeSource".to_string()),
                visibility: LayerVisibility::Visible,
                paint: Some(LayerPaint::Hillshade(HillshadePaint {
                    hillshade_shadow_color: Some(StyleProperty::parse(&serde_json::json!(
                        "#473B24"
                    ))),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        terrain: Some(TerrainSpecification {
            source: "terrainSource".to_string(),
            exaggeration: 1.0,
        }),
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        }),
        sky: Some(SkySpecification::default()),
        center: Some([11.39085, 47.27574]),
        zoom: Some(12.0),
        pitch: Some(70.0),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Add 3D Terrain".to_string()),
        WgpuSettings::default(),
        style(),
        HeadedMapOptions {
            max_pitch_degrees: 85.0,
            ..Default::default()
        },
    )?;
    Ok(())
}

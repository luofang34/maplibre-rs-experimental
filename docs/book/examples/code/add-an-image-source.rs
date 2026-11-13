use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, RasterPaint, StyleLayer, StyleProperty},
        source::{fresh_generation, ImageSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

const BASE_STYLE_URL: &str = "https://tiles.openfreemap.org/styles/bright";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([-75.789, 41.874]);
    style.zoom = Some(5.0);

    style.sources.insert(
        "radar".to_string(),
        Source::Image(ImageSource {
            url: "https://maplibre.org/maplibre-gl-js/docs/assets/radar.gif".to_string(),
            coordinates: [
                [-80.425, 46.437],
                [-71.516, 46.437],
                [-71.516, 37.936],
                [-80.425, 37.936],
            ],
            generation: fresh_generation(),
        }),
    );

    style.layers.push(StyleLayer {
        id: "radar-layer".to_string(),
        type_: "raster".to_string(),
        source: Some("radar".to_string()),
        paint: Some(LayerPaint::Raster(RasterPaint {
            raster_opacity: Some(StyleProperty::Constant(0.85)),
            raster_fade_duration: Some(0),
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
    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Add an Image Source".to_string()),
        WgpuSettings::default(),
        build_style(base),
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

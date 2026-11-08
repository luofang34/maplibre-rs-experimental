use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::Style,
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

const BASE_STYLE_URL: &str = "https://demotiles.maplibre.org/style.json";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([0.0, 0.0]);
    style.zoom = Some(2.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base: Style = maplibre::platform::run_multithreaded(async {
        Ok::<_, reqwest::Error>(reqwest::get(BASE_STYLE_URL).await?.json().await?)
    })?;
    let style = build_style(base);

    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Display a Globe with a Vector Map".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

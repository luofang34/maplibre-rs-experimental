use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{FillExtrusionPaint, LayerPaint, StyleLayer, StyleProperty},
        source::{Source, VectorSource},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
use serde_json::json;

const BASE_STYLE_URL: &str = "https://tiles.openfreemap.org/styles/bright";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([-74.0066, 40.7135]);
    style.zoom = Some(15.5);
    style.pitch = Some(45.0);
    style.bearing = Some(-17.6);

    let building_source = if style.sources.contains_key("openmaptiles") {
        "openmaptiles".to_string()
    } else {
        style.sources.insert(
            "openfreemap".to_string(),
            Source::Vector(VectorSource {
                attribution: None,
                bounds: None,
                maxzoom: None,
                minzoom: None,
                scheme: None,
                tiles: None,
                tile_size: None,
                url: Some("https://tiles.openfreemap.org/planet".to_string()),
            }),
        );
        "openfreemap".to_string()
    };
    style.layers.retain(|layer| layer.id != "building-top");

    // Insert the layer beneath the first symbol layer with labels.
    let label_layer_index = style
        .layers
        .iter()
        .position(|layer| {
            matches!(
                &layer.paint,
                Some(LayerPaint::Symbol(symbol)) if symbol.text_field.is_some()
            )
        })
        .unwrap_or(style.layers.len());

    style.layers.insert(
        label_layer_index,
        StyleLayer {
            id: "3d-buildings".to_string(),
            type_: "fill-extrusion".to_string(),
            source: Some(building_source),
            source_layer: Some("building".to_string()),
            minzoom: Some(14.0),
            filter: Some(json!(["!=", ["get", "hide_3d"], true])),
            paint: Some(LayerPaint::FillExtrusion(FillExtrusionPaint {
                fill_extrusion_color: Some(StyleProperty::parse(&json!([
                    "interpolate",
                    ["linear"],
                    ["get", "render_height"],
                    0,
                    "lightgray",
                    200,
                    "royalblue",
                    400,
                    "lightblue"
                ]))),
                fill_extrusion_height: Some(StyleProperty::parse(&json!(["get", "render_height"]))),
                fill_extrusion_base: Some(StyleProperty::parse(&json!([
                    "get",
                    "render_min_height"
                ]))),
                ..Default::default()
            })),
            ..Default::default()
        },
    );

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
        WinitMapWindowConfig::new("Display Buildings in 3D".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

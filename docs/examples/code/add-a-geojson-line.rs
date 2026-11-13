use std::{path::PathBuf, sync::Arc};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, LayerProperties, LinePaint, StyleLayer, StyleProperty},
        source::{fresh_generation, GeoJsonData, GeoJsonSource, Source},
        Style,
    },
};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
use serde_json::{json, Map};

const BASE_STYLE_URL: &str = "https://tiles.openfreemap.org/styles/bright";

pub fn build_style(mut style: Style) -> Style {
    style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    style.center = Some([-122.486052, 37.830348]);
    style.zoom = Some(15.0);

    style.sources.insert(
        "route".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Inline(Arc::new(json!({
                "type": "Feature",
                "properties": {},
                "geometry": {
                    "type": "LineString",
                    "coordinates": [
                        [-122.48369693756104, 37.83381888486939],
                        [-122.48348236083984, 37.83317489144141],
                        [-122.48339653015138, 37.83270036637107],
                        [-122.48356819152832, 37.832056363179625],
                        [-122.48404026031496, 37.83114119107971],
                        [-122.48404026031496, 37.83049717427869],
                        [-122.48348236083984, 37.829920943955045],
                        [-122.48356819152832, 37.82954808664175],
                        [-122.48507022857666, 37.82944639795659],
                        [-122.48610019683838, 37.82880236636284],
                        [-122.48695850372314, 37.82931081282506],
                        [-122.48700141906738, 37.83080223556934],
                        [-122.48751640319824, 37.83168351665737],
                        [-122.48803138732912, 37.832158048267786],
                        [-122.48888969421387, 37.83297152392784],
                        [-122.48987674713133, 37.83263257682617],
                        [-122.49043464660643, 37.832937629287755],
                        [-122.49125003814696, 37.832429207817725],
                        [-122.49163627624512, 37.832564787218985],
                        [-122.49223709106445, 37.83337825839438],
                        [-122.49378204345702, 37.83368330777276]
                    ]
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
            volatile: false,
            generation: fresh_generation(),
        }),
    );

    style.layers.push(StyleLayer {
        id: "route".to_string(),
        type_: "line".to_string(),
        source: Some("route".to_string()),
        source_layer: None,
        paint: Some(LayerPaint::Line(LinePaint {
            line_color: Some(StyleProperty::parse(&json!("#888"))),
            line_width: Some(StyleProperty::Constant(8.0)),
            ..Default::default()
        })),
        unrecognized: LayerProperties {
            paint: Map::new(),
            layout: Map::from_iter([
                ("line-join".to_string(), json!("round")),
                ("line-cap".to_string(), json!("round")),
            ]),
        },
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
        WinitMapWindowConfig::new("Add a GeoJSON Line".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

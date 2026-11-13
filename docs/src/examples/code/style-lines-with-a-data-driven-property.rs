use std::{path::PathBuf, sync::Arc};

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{
        layer::{LayerPaint, LinePaint, StyleLayer, StyleProperty},
        source::{fresh_generation, GeoJsonData, GeoJsonSource, Source},
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
    style.center = Some([-122.48383155304096, 37.82882682974591]);
    style.zoom = Some(16.0);

    style.sources.insert(
        "lines".to_string(),
        Source::GeoJson(GeoJsonSource {
            data: GeoJsonData::Inline(Arc::new(json!({
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "properties": { "color": "#F7455D" },
                        "geometry": {
                            "type": "LineString",
                            "coordinates": [
                                [-122.4833858013153, 37.829607404976734],
                                [-122.4830961227417, 37.82932776098012],
                                [-122.4830746650696, 37.82932776098012],
                                [-122.48218417167662, 37.82889558180985],
                                [-122.48218417167662, 37.82890193740421],
                                [-122.48221099376678, 37.82868372835086],
                                [-122.4822163581848, 37.82868372835086],
                                [-122.48205006122589, 37.82801003030873]
                            ]
                        }
                    },
                    {
                        "type": "Feature",
                        "properties": { "color": "#33C9EB" },
                        "geometry": {
                            "type": "LineString",
                            "coordinates": [
                                [-122.48393028974533, 37.829471820141016],
                                [-122.48395174741744, 37.82940826466351],
                                [-122.48395174741744, 37.829412501697064],
                                [-122.48423874378203, 37.829357420242125],
                                [-122.48422533273697, 37.829361657278575],
                                [-122.48459815979002, 37.8293425906126],
                                [-122.48458743095398, 37.8293447091313],
                                [-122.4847564101219, 37.82932776098012],
                                [-122.48474299907684, 37.829331998018276],
                                [-122.4849334359169, 37.829298101706186],
                                [-122.48492807149889, 37.82930022022615],
                                [-122.48509705066681, 37.82920488676767],
                                [-122.48509168624878, 37.82920912381288],
                                [-122.48520433902739, 37.82905870855876],
                                [-122.48519897460936, 37.82905870855876],
                                [-122.4854403734207, 37.828594749716714],
                                [-122.48543500900269, 37.82860534241688],
                                [-122.48571664094925, 37.82808206121068],
                                [-122.48570591211319, 37.82809689109353],
                                [-122.4858346581459, 37.82797189627337],
                                [-122.48582661151886, 37.82797825194729],
                                [-122.4859634041786, 37.82788503534145],
                                [-122.48595803976059, 37.82788927246246],
                                [-122.48605459928514, 37.82786596829394]
                            ]
                        }
                    }
                ]
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
        id: "lines".to_string(),
        type_: "line".to_string(),
        source: Some("lines".to_string()),
        source_layer: None,
        paint: Some(LayerPaint::Line(LinePaint {
            line_width: Some(StyleProperty::Constant(3.0)),
            // Use a get expression to set the line-color to a feature property value.
            line_color: Some(StyleProperty::parse(&json!(["get", "color"]))),
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
    let style = build_style(base);

    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Style Lines with a Data-Driven Property".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions::default(),
    )?;
    Ok(())
}

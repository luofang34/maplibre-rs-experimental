#![allow(clippy::expect_used)]

use super::prioritize_remote_sources;
use crate::{
    io::{
        source_type::{GeoJsonTileSource, SourceType, TessellateSource},
        tile_sources::SourceLayerGroup,
    },
    style::source::GeoJsonSource,
};

fn group(name: &str, source: SourceType) -> SourceLayerGroup {
    SourceLayerGroup {
        source_name: Some(name.to_owned()),
        source,
        layers: Vec::new(),
        min_zoom: None,
        max_zoom: None,
        bounds: None,
    }
}

#[test]
fn prioritize_remote_sources_orders_remote_before_geojson() {
    let geojson: GeoJsonSource = serde_json::from_value(serde_json::json!({
        "type": "geojson",
        "data": { "type": "FeatureCollection", "features": [] }
    }))
    .expect("valid geojson source");
    let mut groups = vec![
        group(
            "sensors",
            SourceType::GeoJson(Box::new(GeoJsonTileSource {
                name: "sensors".into(),
                source: geojson,
            })),
        ),
        group(
            "openmaptiles",
            SourceType::Tessellate(TessellateSource::default()),
        ),
    ];

    prioritize_remote_sources(&mut groups);
    let ordered: Vec<_> = groups
        .iter()
        .filter_map(|g| g.source_name.as_deref())
        .collect();
    assert_eq!(ordered, vec!["openmaptiles", "sensors"]);
}

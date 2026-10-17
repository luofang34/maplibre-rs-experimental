//! GeoJSON data a style change replaces refreshes the loaded tiles it changes by itself.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{
    super::fixture::{Fixture, Kind},
    tests::{geojson_style, WORLD_POLYGON},
};
use crate::style::{mutation::StyleChange, source::GeoJsonData};

fn replace(test: &mut Fixture, data: &Value) {
    test.context
        .mutate_style(|style| {
            style
                .set_geojson_data("shapes", GeoJsonData::Inline(Arc::new(data.clone())))
                .expect("set data");
            Ok(StyleChange::default())
        })
        .expect("style change");
}

#[tokio::test]
async fn data_set_in_a_style_change_refreshes_only_a_tile_it_changes() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let polygon: Value = serde_json::from_str(WORLD_POLYGON).expect("polygon");
    test.context.style = geojson_style(polygon.clone());
    test.frame(0);
    test.receive().await;
    assert!(test.loaded());

    replace(&mut test, &polygon);
    test.frame(1);
    assert_eq!(
        test.kernel.apc().pending(),
        0,
        "the same features leave the tile as it is"
    );

    replace(
        &mut test,
        &json!({"type": "FeatureCollection", "features": []}),
    );
    test.frame(2);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "the tile the polygon covered is requested again"
    );
    assert!(
        test.loaded(),
        "and keeps the polygon until the new tile arrives"
    );
    test.receive().await;
    assert!(!test.loaded());
}

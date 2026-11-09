// The same query from Rust in the browser, through the handle `web::run_map` resolves to.
use maplibre::{
    map::handle::MapHandle,
    query::QueriedFeature,
    sdf::query::{QueryGeometry, QueryOptions},
    style::Style,
};
use web::{CurrentEnvironment, JSError};

pub fn build_style(mut style: Style) -> Style {
    style.center = Some([-74.006, 40.7128]);
    style.zoom = Some(13.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

/// The features drawn under a pointer `[x, y]` in CSS pixels from the canvas's top left,
/// topmost first.
pub async fn features_under(
    map: &MapHandle<CurrentEnvironment>,
    point: [f64; 2],
) -> Result<Vec<QueriedFeature>, JSError> {
    let features = map
        .call(move |map| {
            map.query_rendered_features(QueryGeometry::Point(point), &QueryOptions::default())
        })
        .await??;
    Ok(features)
}

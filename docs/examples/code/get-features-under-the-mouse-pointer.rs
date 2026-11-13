// Query rendered features through `MapHandle` (`web::run_map` on web or `HeadedMapHost` natively).
use maplibre::{
    map::handle::MapHandle,
    query::QueriedFeature,
    sdf::query::{QueryGeometry, QueryOptions},
    style::Style,
};
#[cfg(not(target_arch = "wasm32"))]
use maplibre_winit::NativeEnvironment as CurrentEnvironment;
#[cfg(target_arch = "wasm32")]
use web::{CurrentEnvironment, JSError};

#[cfg(not(target_arch = "wasm32"))]
type JSError = Box<dyn std::error::Error>;

pub fn build_style(mut style: Style) -> Style {
    style.center = Some([-74.006, 40.7128]);
    style.zoom = Some(13.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

/// The features drawn under a pointer `[x, y]` in logical pixels from the viewport's top left,
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

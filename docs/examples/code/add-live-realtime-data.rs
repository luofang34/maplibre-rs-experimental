// The same updates from Rust in the browser, through the handle `web::run_map` resolves to.
use std::sync::Arc;

use maplibre::{
    coords::LatLon,
    map::handle::MapHandle,
    render::view_state::{CameraOptions, FlightCurve},
    style::{source::GeoJsonData, Style},
};
use serde::Deserialize;
use serde_json::json;
use web::CurrentEnvironment;

const ISS_URL: &str = "https://api.wheretheiss.at/v1/satellites/25544";

pub fn build_style(base: Style) -> Result<Style, serde_json::Error> {
    let mut document = serde_json::to_value(base)?;
    document["center"] = json!([0, 0]);
    document["zoom"] = json!(2);
    document["sources"]["iss"] = json!({
        "type": "geojson",
        "data": {"type": "FeatureCollection", "features": []}
    });
    if let Some(layers) = document["layers"].as_array_mut() {
        layers.push(json!({
            "id": "iss",
            "type": "circle",
            "source": "iss",
            "paint": {"circle-radius": 8, "circle-color": "#e55e5e"}
        }));
    }
    let mut style: Style = serde_json::from_value(document)?;
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

#[derive(Deserialize)]
struct Position {
    longitude: f64,
    latitude: f64,
}

/// Moves the point to where the International Space Station is now, and the camera with it.
pub async fn update(map: &MapHandle<CurrentEnvironment>) -> Result<(), Box<dyn std::error::Error>> {
    let Position {
        longitude,
        latitude,
    } = reqwest::get(ISS_URL).await?.json().await?;
    let point = json!({
        "type": "Feature",
        "properties": {},
        "geometry": {"type": "Point", "coordinates": [longitude, latitude]}
    });
    let camera = CameraOptions {
        center: Some(LatLon::new(latitude, longitude)),
        ..Default::default()
    };
    map.call(move |map| {
        map.set_geojson_data("iss", GeoJsonData::Inline(Arc::new(point)))?;
        map.fly_to(
            camera,
            FlightCurve::Ease {
                duration: std::time::Duration::from_secs(1),
            },
        )
    })
    .await??;
    Ok(())
}

/// Updates the point every two seconds for as long as the map runs.
pub async fn follow(map: MapHandle<CurrentEnvironment>) {
    while !map.is_closed() {
        if let Err(error) = update(&map).await {
            tracing::warn!(%error, "the station's position could not be shown");
        }
        let delay = js_sys::Promise::new(&mut |resolve, _| {
            if let Some(window) = web_sys::window() {
                window
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 2000)
                    .ok();
            }
        });
        wasm_bindgen_futures::JsFuture::from(delay).await.ok();
    }
}

// Driven through a `MapHandle`: `web::run_map` (or `HeadedMapHost` on desktop) yields a handle
// whose commands the map applies between frames.
use maplibre::{
    context::FlightOutcome,
    coords::LatLon,
    map::handle::MapHandle,
    render::view_state::{CameraOptions, FlightCurve, FlyOptions},
    style::Style,
};
#[cfg(not(target_arch = "wasm32"))]
use maplibre_winit::NativeEnvironment as CurrentEnvironment;
#[cfg(target_arch = "wasm32")]
use web::{CurrentEnvironment, JSError, WebMapHost};

#[cfg(not(target_arch = "wasm32"))]
type JSError = Box<dyn std::error::Error>;

pub fn build_style(mut style: Style) -> Style {
    style.center = Some([-74.5, 40.0]);
    style.zoom = Some(9.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

/// Flies an active map handle to `[lng, lat]` and reports whether the camera arrived there.
pub async fn fly_camera(
    map: &MapHandle<CurrentEnvironment>,
    [lng, lat]: [f64; 2],
) -> Result<bool, JSError> {
    let destination = CameraOptions {
        center: Some(LatLon::new(lat, lng)),
        ..Default::default()
    };
    let flight = map
        .call(move |map| map.fly_to(destination, FlightCurve::Fly(FlyOptions::default())))
        .await??;
    // Asked again after every frame until the move ends.
    let arrived = map
        .until(move |map| match map.flight_outcome(flight) {
            FlightOutcome::Flying => None,
            outcome => Some(outcome == FlightOutcome::Arrived),
        })
        .await?;
    Ok(arrived)
}

/// Starts the map in the browser, flies to `[lng, lat]` and tells whether the camera arrived.
#[cfg(target_arch = "wasm32")]
pub async fn fly(
    new_worker: js_sys::Function,
    base: Style,
    destination: [f64; 2],
) -> Result<bool, JSError> {
    let map = web::run_map(new_worker, build_style(base), WebMapHost::default()).await?;
    fly_camera(&map, destination).await
}

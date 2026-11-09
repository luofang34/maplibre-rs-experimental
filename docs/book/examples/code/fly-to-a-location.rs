// The same map driven from Rust in the browser: `web::run_map` resolves to a handle whose
// commands the map applies between frames.
use maplibre::{
    context::FlightOutcome,
    coords::LatLon,
    render::view_state::{CameraOptions, FlightCurve, FlyOptions},
    style::Style,
};
use web::{JSError, WebMapHost};

pub fn build_style(mut style: Style) -> Style {
    style.center = Some([-74.5, 40.0]);
    style.zoom = Some(9.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

/// Starts the map, flies to `[lng, lat]` and tells whether the camera arrived there.
pub async fn fly(
    new_worker: js_sys::Function,
    base: Style,
    [lng, lat]: [f64; 2],
) -> Result<bool, JSError> {
    let map = web::run_map(new_worker, build_style(base), WebMapHost::default()).await?;
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

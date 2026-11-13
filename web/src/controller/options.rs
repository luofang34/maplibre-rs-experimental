//! The shapes GL JS gives camera, animation and query arguments, read from JSON and turned
//! into the core types once.

use std::time::Duration;

use maplibre::{
    coords::LatLon,
    query::QueriedFeature,
    render::view_state::{CameraOptions, FlightCurve, FlyOptions},
    sdf::query::{QueryGeometry, QueryOptions},
};
use serde::{Deserialize, Serialize};

/// A GL JS `LngLatLike`: `[lng, lat]` or `{lng, lat}`.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq)]
#[serde(untagged)]
pub(crate) enum LngLat {
    Array([f64; 2]),
    Object { lng: f64, lat: f64 },
}

impl From<LngLat> for LatLon {
    fn from(value: LngLat) -> Self {
        match value {
            LngLat::Array([lng, lat]) | LngLat::Object { lng, lat } => LatLon::new(lat, lng),
        }
    }
}

/// GL JS `CameraOptions`, angles in degrees; what is left out keeps its value.
#[derive(Deserialize, Debug, Default, Clone, Copy)]
pub(crate) struct Camera {
    center: Option<LngLat>,
    zoom: Option<f64>,
    bearing: Option<f64>,
    pitch: Option<f64>,
    roll: Option<f64>,
    elevation: Option<f64>,
}

impl From<Camera> for CameraOptions {
    fn from(camera: Camera) -> Self {
        CameraOptions {
            center: camera.center.map(LatLon::from),
            zoom: camera.zoom,
            bearing: camera.bearing.map(cgmath::Deg),
            pitch: camera.pitch.map(cgmath::Deg),
            roll: camera.roll.map(cgmath::Deg),
            elevation: camera.elevation,
        }
    }
}

/// GL JS `EaseToOptions` and `FlyToOptions`: a camera with how to move to it.
#[derive(Deserialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Animation {
    #[serde(flatten)]
    camera: Camera,
    /// Milliseconds the move takes.
    duration: Option<f64>,
    curve: Option<f64>,
    speed: Option<f64>,
    min_zoom: Option<f64>,
}

/// GL JS `easeTo`'s default length.
const EASE_DURATION_MS: f64 = 500.0;

fn millis(ms: f64) -> Duration {
    Duration::from_secs_f64(ms.max(0.0) / 1000.0)
}

impl Animation {
    /// The camera and an `easeTo` curve.
    pub(crate) fn ease(self) -> (CameraOptions, FlightCurve) {
        let duration = millis(self.duration.unwrap_or(EASE_DURATION_MS));
        (self.camera.into(), FlightCurve::Ease { duration })
    }

    /// The camera and a `flyTo` curve, with GL JS's defaults for what is left out.
    pub(crate) fn fly(self) -> (CameraOptions, FlightCurve) {
        let defaults = FlyOptions::default();
        let options = FlyOptions {
            curve: self.curve.unwrap_or(defaults.curve),
            speed: self.speed.unwrap_or(defaults.speed),
            duration: self.duration.map(millis),
            min_zoom: self.min_zoom.unwrap_or(defaults.min_zoom),
        };
        (self.camera.into(), FlightCurve::Fly(options))
    }
}

/// Where the camera is, in the shape of GL JS `CameraOptions`.
#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct CameraState {
    pub center: [f64; 2],
    pub zoom: f64,
    pub bearing: f64,
    pub pitch: f64,
}

impl CameraState {
    pub(crate) fn of(context: &maplibre::context::MapContext) -> Self {
        let view = &context.view_state;
        let center = view.center_lat_lon();
        let camera = view.camera();
        Self {
            center: [center.longitude, center.latitude],
            zoom: view.zoom().value(),
            bearing: cgmath::Deg::from(camera.get_bearing()).0,
            pitch: cgmath::Deg::from(camera.get_pitch()).0,
        }
    }
}

/// A GL JS `PointLike` or a pair of them for a box, in CSS pixels from the canvas's top left.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(untagged)]
pub(crate) enum ScreenGeometry {
    Point([f64; 2]),
    Box([[f64; 2]; 2]),
}

impl From<ScreenGeometry> for QueryGeometry {
    /// CSS pixels are the logical pixels the map's view and query measure in, since the
    /// window's scale factor is the page's device pixel ratio.
    fn from(geometry: ScreenGeometry) -> Self {
        match geometry {
            ScreenGeometry::Point(point) => QueryGeometry::Point(point),
            ScreenGeometry::Box([min, max]) => QueryGeometry::Box { min, max },
        }
    }
}

/// The options of GL JS `queryRenderedFeatures`.
#[derive(Deserialize, Debug, Default, Clone)]
pub(crate) struct Query {
    layers: Option<Vec<String>>,
    filter: Option<serde_json::Value>,
}

impl From<Query> for QueryOptions {
    fn from(query: Query) -> Self {
        QueryOptions {
            layers: query.layers,
            filter: query.filter,
        }
    }
}

/// A queried feature in the shape of GL JS `MapGeoJSONFeature`, without its geometry.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Feature {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    properties: std::collections::BTreeMap<String, serde_json::Value>,
    state: std::collections::BTreeMap<String, serde_json::Value>,
    layer: LayerRef,
    source: Option<String>,
    source_layer: String,
    geometry_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
struct LayerRef {
    id: String,
}

impl From<QueriedFeature> for Feature {
    fn from(feature: QueriedFeature) -> Self {
        Self {
            kind: "Feature",
            id: feature.id,
            properties: feature.properties,
            state: feature.state,
            layer: LayerRef { id: feature.layer },
            source: feature.source,
            source_layer: feature.source_layer,
            geometry_type: feature.geometry_type,
            text: feature.text,
        }
    }
}

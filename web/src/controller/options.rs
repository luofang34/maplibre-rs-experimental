//! The shapes GL JS gives camera, animation and query arguments, read from JSON and turned
//! into the core types once.

use std::time::Duration;

use maplibre::{
    coords::LatLon,
    query::QueriedFeature,
    render::view_state::{CameraOptions, FlightCurve, FlyOptions},
    sdf::query::{QueryGeometry, QueryOptions},
};
use serde::Serialize;

fn opt_f64(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, serde_json::Error> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("{key}: expected number"))),
    }
}

fn opt_lng_lat(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<LngLat>, serde_json::Error> {
    match map.get("center") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => LngLat::from_json_value(value).map(Some),
    }
}

/// A GL JS `LngLatLike`: `[lng, lat]` or `{lng, lat}`.
#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
pub(crate) enum LngLat {
    Array([f64; 2]),
    Object { lng: f64, lat: f64 },
}

impl LngLat {
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        match value {
            serde_json::Value::Array(items) => {
                let [lng, lat] = items.as_slice() else {
                    return Err(serde::de::Error::custom("expected [lng, lat]"));
                };
                let lng = lng
                    .as_f64()
                    .ok_or_else(|| serde::de::Error::custom("expected numeric longitude"))?;
                let lat = lat
                    .as_f64()
                    .ok_or_else(|| serde::de::Error::custom("expected numeric latitude"))?;
                Ok(Self::Array([lng, lat]))
            }
            serde_json::Value::Object(map) => {
                let lng = map
                    .get("lng")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| serde::de::Error::custom("missing or invalid `lng`"))?;
                let lat = map
                    .get("lat")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| serde::de::Error::custom("missing or invalid `lat`"))?;
                Ok(Self::Object { lng, lat })
            }
            _ => Err(serde::de::Error::custom(
                "expected [lng, lat] or {lng, lat}",
            )),
        }
    }
}

impl From<LngLat> for LatLon {
    fn from(value: LngLat) -> Self {
        match value {
            LngLat::Array([lng, lat]) | LngLat::Object { lng, lat } => LatLon::new(lat, lng),
        }
    }
}

/// GL JS `CameraOptions`, angles in degrees; what is left out keeps its value.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Camera {
    center: Option<LngLat>,
    zoom: Option<f64>,
    bearing: Option<f64>,
    pitch: Option<f64>,
    roll: Option<f64>,
    elevation: Option<f64>,
}

impl Camera {
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom("expected camera options object"));
        };
        Ok(Self {
            center: opt_lng_lat(map)?,
            zoom: opt_f64(map, "zoom")?,
            bearing: opt_f64(map, "bearing")?,
            pitch: opt_f64(map, "pitch")?,
            roll: opt_f64(map, "roll")?,
            elevation: opt_f64(map, "elevation")?,
        })
    }
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

pub(crate) fn merge_camera(base: CameraOptions, next: CameraOptions) -> CameraOptions {
    CameraOptions {
        center: next.center.or(base.center),
        zoom: next.zoom.or(base.zoom),
        bearing: next.bearing.or(base.bearing),
        pitch: next.pitch.or(base.pitch),
        roll: next.roll.or(base.roll),
        elevation: next.elevation.or(base.elevation),
    }
}

/// Fast-path reader for plain JS camera objects (`{center, zoom, bearing, pitch, roll, elevation}`),
/// avoiding `JSON.stringify` + `serde_json` allocations on 60–120 Hz `jumpTo` calls.
pub(crate) fn try_read_camera_fast(value: &wasm_bindgen::JsValue) -> Option<CameraOptions> {
    use wasm_bindgen::JsValue;
    if !value.is_object() || value.is_null() || js_sys::Array::is_array(value) {
        return None;
    }
    let prop = |key: &str| js_sys::Reflect::get(value, &JsValue::from_str(key)).ok();
    let num = |key: &str| -> Option<Option<f64>> {
        let v = prop(key)?;
        if v.is_undefined() || v.is_null() {
            Some(None)
        } else {
            v.as_f64().map(Some)
        }
    };

    let center_val = prop("center")?;
    let center = if center_val.is_undefined() || center_val.is_null() {
        None
    } else if js_sys::Array::is_array(&center_val) {
        let arr = js_sys::Array::from(&center_val);
        if arr.length() != 2 {
            return None;
        }
        Some(LatLon::new(arr.get(1).as_f64()?, arr.get(0).as_f64()?))
    } else if center_val.is_object() {
        let lng = js_sys::Reflect::get(&center_val, &JsValue::from_str("lng"))
            .ok()?
            .as_f64()?;
        let lat = js_sys::Reflect::get(&center_val, &JsValue::from_str("lat"))
            .ok()?
            .as_f64()?;
        Some(LatLon::new(lat, lng))
    } else {
        return None;
    };

    Some(CameraOptions {
        center,
        zoom: num("zoom")?,
        bearing: num("bearing")?.map(cgmath::Deg),
        pitch: num("pitch")?.map(cgmath::Deg),
        roll: num("roll")?.map(cgmath::Deg),
        elevation: num("elevation")?,
    })
}

/// GL JS `EaseToOptions` and `FlyToOptions`: a camera with how to move to it.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Animation {
    center: Option<LngLat>,
    zoom: Option<f64>,
    bearing: Option<f64>,
    pitch: Option<f64>,
    roll: Option<f64>,
    elevation: Option<f64>,
    /// Milliseconds the move takes.
    duration: Option<f64>,
    curve: Option<f64>,
    speed: Option<f64>,
    min_zoom: Option<f64>,
}

impl Animation {
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom(
                "expected animation options object",
            ));
        };
        Ok(Self {
            center: opt_lng_lat(map)?,
            zoom: opt_f64(map, "zoom")?,
            bearing: opt_f64(map, "bearing")?,
            pitch: opt_f64(map, "pitch")?,
            roll: opt_f64(map, "roll")?,
            elevation: opt_f64(map, "elevation")?,
            duration: opt_f64(map, "duration")?,
            curve: opt_f64(map, "curve")?,
            speed: opt_f64(map, "speed")?,
            min_zoom: opt_f64(map, "minZoom")?,
        })
    }
}

/// GL JS `easeTo`'s default length.
const EASE_DURATION_MS: f64 = 500.0;

fn millis(ms: f64) -> Duration {
    Duration::from_secs_f64(ms.max(0.0) / 1000.0)
}

impl Animation {
    fn camera(&self) -> Camera {
        Camera {
            center: self.center,
            zoom: self.zoom,
            bearing: self.bearing,
            pitch: self.pitch,
            roll: self.roll,
            elevation: self.elevation,
        }
    }

    /// The camera and an `easeTo` curve.
    pub(crate) fn ease(self) -> (CameraOptions, FlightCurve) {
        let duration = millis(self.duration.unwrap_or(EASE_DURATION_MS));
        (self.camera().into(), FlightCurve::Ease { duration })
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
        (self.camera().into(), FlightCurve::Fly(options))
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ScreenGeometry {
    Point([f64; 2]),
    Box([[f64; 2]; 2]),
}

impl ScreenGeometry {
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some([first, second]) = value.as_array().map(Vec::as_slice) else {
            return Err(serde::de::Error::custom(
                "expected [x, y] or [[x1, y1], [x2, y2]]",
            ));
        };
        if let (Some(x), Some(y)) = (first.as_f64(), second.as_f64()) {
            return Ok(Self::Point([x, y]));
        }
        let pair = |v: &serde_json::Value| -> Result<[f64; 2], serde_json::Error> {
            let Some([a, b]) = v.as_array().map(Vec::as_slice) else {
                return Err(serde::de::Error::custom("expected [x, y]"));
            };
            let a = a
                .as_f64()
                .ok_or_else(|| serde::de::Error::custom("expected numeric coordinate"))?;
            let b = b
                .as_f64()
                .ok_or_else(|| serde::de::Error::custom("expected numeric coordinate"))?;
            Ok([a, b])
        };
        Ok(Self::Box([pair(first)?, pair(second)?]))
    }
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
#[derive(Debug, Default, Clone)]
pub(crate) struct Query {
    layers: Option<Vec<String>>,
    filter: Option<serde_json::Value>,
}

impl Query {
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom("expected query options object"));
        };
        let layers = match map.get("layers") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => {
                let items = v
                    .as_array()
                    .ok_or_else(|| serde::de::Error::custom("layers: expected array"))?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let s = item
                        .as_str()
                        .ok_or_else(|| serde::de::Error::custom("layers: expected string"))?;
                    out.push(s.to_owned());
                }
                Some(out)
            }
        };
        let filter = map.get("filter").filter(|v| !v.is_null()).cloned();
        Ok(Self { layers, filter })
    }
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

pub(crate) trait FromJsJson: Sized {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error>;
}

impl FromJsJson for serde_json::Value {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Ok(value.clone())
    }
}

impl FromJsJson for maplibre::style::layer::StyleLayer {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

impl FromJsJson for maplibre::style::source::GeoJsonData {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Ok(Self::from_json_value(value))
    }
}

impl FromJsJson for maplibre::geojson::update::GeoJsonDiff {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

impl FromJsJson for Camera {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

impl FromJsJson for Animation {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

impl FromJsJson for ScreenGeometry {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

impl FromJsJson for Query {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::from_json_value(value)
    }
}

#[inline(never)]
fn parse_js_json_value(
    value: &wasm_bindgen::JsValue,
    what: &'static str,
) -> Result<serde_json::Value, crate::error::JSError> {
    let json = js_sys::JSON::stringify(value)
        .ok()
        .and_then(|json| json.as_string())
        .ok_or(crate::error::ArgumentError::NotJson { what })?;
    serde_json::from_slice(json.as_bytes()).map_err(|source| {
        crate::error::JSError::from(crate::error::ArgumentError::Invalid { what, source })
    })
}

/// Reads a JavaScript argument through JSON without `MapOnlyValueRef` monomorphizations.
pub(crate) fn read<T: FromJsJson>(
    value: &wasm_bindgen::JsValue,
    what: &'static str,
) -> Result<T, crate::error::JSError> {
    let parsed = parse_js_json_value(value, what)?;
    T::from_json_value(&parsed).map_err(|source| {
        crate::error::JSError::from(crate::error::ArgumentError::Invalid { what, source })
    })
}

/// Reads an argument that may be `undefined` or `null`.
pub(crate) fn read_optional<T: FromJsJson>(
    value: &wasm_bindgen::JsValue,
    what: &'static str,
) -> Result<Option<T>, crate::error::JSError> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    read(value, what).map(Some)
}

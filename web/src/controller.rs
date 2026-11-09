//! The map a page started, controlled from JavaScript with GL JS's method names and argument
//! shapes. Every method queues a command the map applies between frames, in call order, and
//! returns a promise that settles once it has.

use std::{cell::Cell, future::Future, rc::Rc};

use maplibre::{
    context::FlightOutcome,
    map::handle::{MapHandle, Reply},
    render::view_state::{CameraOptions, FlightCurve},
    sdf::query::QueryGeometry,
    style::{mutation::StyleChange, source::GeoJsonData},
};
use serde::{de::DeserializeOwned, Serialize};
use wasm_bindgen::prelude::*;

use crate::{
    application::MapType,
    environment::CurrentEnvironment,
    error::{ArgumentError, JSError},
};

mod options;
use options::{Animation, Camera, CameraState, Feature, Query, ScreenGeometry};

/// A running map, as `startMapLibre` resolves it.
#[wasm_bindgen]
#[derive(Clone)]
pub struct MapController {
    handle: MapHandle<CurrentEnvironment>,
}

impl std::fmt::Debug for MapController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapController")
            .field("closed", &self.handle.is_closed())
            .finish()
    }
}

impl MapController {
    /// Controls the map `handle` sends commands to.
    pub fn new(handle: MapHandle<CurrentEnvironment>) -> Self {
        Self { handle }
    }

    /// The handle underneath, for a Rust host that reads or changes what JavaScript cannot.
    pub fn handle(&self) -> &MapHandle<CurrentEnvironment> {
        &self.handle
    }

    /// Queues `command` and returns a promise of what it returns, encoded as JSON.
    fn call<R: Serialize + 'static>(
        &self,
        command: impl FnOnce(&mut MapType) -> Result<R, JSError> + 'static,
    ) -> js_sys::Promise {
        settle(flatten(self.handle.call(command)))
    }

    /// Queues a style change; GL JS's style setters return nothing.
    fn change(
        &self,
        change: impl FnOnce(&mut MapType) -> Result<StyleChange, maplibre::map::MapError> + 'static,
    ) -> js_sys::Promise {
        self.call(move |map| change(map).map(drop).map_err(JSError::from))
    }

    /// Starts a move along `curve` and returns a promise of whether it arrived.
    fn fly(&self, (camera, curve): (CameraOptions, FlightCurve)) -> js_sys::Promise {
        let flight = Rc::new(Cell::new(None));
        let started = {
            let flight = flight.clone();
            self.handle.call(move |map| {
                flight.set(Some(map.fly_to(camera, curve)?));
                Ok::<_, JSError>(())
            })
        };
        // Queued right behind the start, so it follows this move and no later one.
        let arrived = self.handle.until(move |map| match flight.get() {
            None => Some(false),
            Some(id) => match map.flight_outcome(id) {
                FlightOutcome::Flying => None,
                FlightOutcome::Arrived => Some(true),
                FlightOutcome::Interrupted | FlightOutcome::Superseded => Some(false),
            },
        });
        settle(async move {
            started.await??;
            Ok(arrived.await?)
        })
    }
}

#[wasm_bindgen]
impl MapController {
    /// Moves the camera at once, as GL JS `jumpTo`.
    #[wasm_bindgen(js_name = jumpTo)]
    pub fn jump_to(&self, options: JsValue) -> js_sys::Promise {
        match read::<Camera>(&options, "camera options") {
            Ok(camera) => self.call(move |map| map.jump_to(camera.into()).map_err(JSError::from)),
            Err(error) => rejected(error),
        }
    }

    /// Eases the camera to `options`, as GL JS `easeTo`; resolves to whether it arrived.
    #[wasm_bindgen(js_name = easeTo)]
    pub fn ease_to(&self, options: JsValue) -> js_sys::Promise {
        match read::<Animation>(&options, "ease options") {
            Ok(animation) => self.fly(animation.ease()),
            Err(error) => rejected(error),
        }
    }

    /// Flies the camera to `options`, as GL JS `flyTo`; resolves to whether it arrived.
    #[wasm_bindgen(js_name = flyTo)]
    pub fn fly_to(&self, options: JsValue) -> js_sys::Promise {
        match read::<Animation>(&options, "fly options") {
            Ok(animation) => self.fly(animation.fly()),
            Err(error) => rejected(error),
        }
    }

    /// Stops a camera move where the camera is, as GL JS `stop`.
    pub fn stop(&self) -> js_sys::Promise {
        self.call(|map| {
            map.stop_flight();
            Ok(())
        })
    }

    /// The camera's center as `[lng, lat]`, zoom, bearing and pitch.
    #[wasm_bindgen(js_name = getCamera)]
    pub fn get_camera(&self) -> js_sys::Promise {
        self.call(|map| Ok(CameraState::of(map.context()?)))
    }

    /// Sets a paint property, as GL JS `setPaintProperty`; `null` restores its default.
    #[wasm_bindgen(js_name = setPaintProperty)]
    pub fn set_paint_property(
        &self,
        layer: String,
        name: String,
        value: JsValue,
    ) -> js_sys::Promise {
        match read(&value, "paint value") {
            Ok(value) => self.change(move |map| map.set_paint_property(&layer, &name, value)),
            Err(error) => rejected(error),
        }
    }

    /// Sets a layout property, as GL JS `setLayoutProperty`; `null` restores its default.
    #[wasm_bindgen(js_name = setLayoutProperty)]
    pub fn set_layout_property(
        &self,
        layer: String,
        name: String,
        value: JsValue,
    ) -> js_sys::Promise {
        match read(&value, "layout value") {
            Ok(value) => self.change(move |map| map.set_layout_property(&layer, &name, value)),
            Err(error) => rejected(error),
        }
    }

    /// Sets a layer's filter, as GL JS `setFilter`; `null` or nothing removes it.
    #[wasm_bindgen(js_name = setFilter)]
    pub fn set_filter(&self, layer: String, filter: JsValue) -> js_sys::Promise {
        match read_optional(&filter, "filter") {
            Ok(filter) => self.change(move |map| map.set_filter(&layer, filter)),
            Err(error) => rejected(error),
        }
    }

    /// Adds a layer before the layer `before_id`, or on top, as GL JS `addLayer`.
    #[wasm_bindgen(js_name = addLayer)]
    pub fn add_layer(&self, layer: JsValue, before_id: Option<String>) -> js_sys::Promise {
        match read(&layer, "layer") {
            Ok(layer) => self.change(move |map| map.add_layer(layer, before_id.as_deref())),
            Err(error) => rejected(error),
        }
    }

    /// Removes a layer, as GL JS `removeLayer`.
    #[wasm_bindgen(js_name = removeLayer)]
    pub fn remove_layer(&self, id: String) -> js_sys::Promise {
        self.change(move |map| map.remove_layer(&id))
    }

    /// Sets a `global-state` value, as GL JS `setGlobalStateProperty`; `null` restores the
    /// declared default.
    #[wasm_bindgen(js_name = setGlobalStateProperty)]
    pub fn set_global_state_property(&self, name: String, value: JsValue) -> js_sys::Promise {
        match read_optional(&value, "global state value") {
            Ok(value) => self.call(move |map| {
                map.set_global_state(&name, value.unwrap_or_default());
                Ok(())
            }),
            Err(error) => rejected(error),
        }
    }

    /// Every `global-state` key with its value, as GL JS `getGlobalState`.
    #[wasm_bindgen(js_name = getGlobalState)]
    pub fn get_global_state(&self) -> js_sys::Promise {
        self.call(|map| Ok(map.global_state()))
    }

    /// Replaces a GeoJSON source's data with a document or a URL, as GL JS
    /// `GeoJSONSource.setData`.
    #[wasm_bindgen(js_name = setGeoJsonData)]
    pub fn set_geojson_data(&self, source: String, data: JsValue) -> js_sys::Promise {
        match read::<GeoJsonData>(&data, "GeoJSON data") {
            Ok(data) => self.call(move |map| Ok(map.set_geojson_data(&source, data)?)),
            Err(error) => rejected(error),
        }
    }

    /// Adds, changes and removes features of an inline GeoJSON source, as GL JS
    /// `GeoJSONSource.updateData`.
    #[wasm_bindgen(js_name = updateGeoJsonData)]
    pub fn update_geojson_data(&self, source: String, diff: JsValue) -> js_sys::Promise {
        match read(&diff, "GeoJSON diff") {
            Ok(diff) => self.call(move |map| Ok(map.update_geojson_data(&source, &diff)?)),
            Err(error) => rejected(error),
        }
    }

    /// Features drawn under a point `[x, y]` or a box `[[x0, y0], [x1, y1]]` in CSS pixels
    /// from the canvas's top left, or the whole canvas when left out, topmost first; as GL JS
    /// `queryRenderedFeatures`.
    #[wasm_bindgen(js_name = queryRenderedFeatures)]
    pub fn query_rendered_features(&self, geometry: JsValue, options: JsValue) -> js_sys::Promise {
        let arguments =
            read_optional::<ScreenGeometry>(&geometry, "query geometry").and_then(|geometry| {
                Ok((geometry, read_optional::<Query>(&options, "query options")?))
            });
        let (geometry, query) = match arguments {
            Ok(arguments) => arguments,
            Err(error) => return rejected(error),
        };
        self.call(move |map| {
            let geometry = match geometry {
                Some(geometry) => QueryGeometry::from(geometry),
                None => {
                    let view = &map.context()?.view_state;
                    QueryGeometry::Box {
                        min: [0.0, 0.0],
                        max: [view.width(), view.height()],
                    }
                }
            };
            let features =
                map.query_rendered_features(geometry, &query.unwrap_or_default().into())?;
            Ok(features.into_iter().map(Feature::from).collect::<Vec<_>>())
        })
    }
}

/// Reads a JavaScript argument through JSON, which every argument of the map's API is.
fn read<T: DeserializeOwned>(value: &JsValue, what: &'static str) -> Result<T, JSError> {
    let json = js_sys::JSON::stringify(value)
        .ok()
        .and_then(|json| json.as_string())
        .ok_or(ArgumentError::NotJson { what })?;
    Ok(serde_json::from_str(&json).map_err(|source| ArgumentError::Invalid { what, source })?)
}

/// Reads an argument that may be `undefined` or `null`.
fn read_optional<T: DeserializeOwned>(
    value: &JsValue,
    what: &'static str,
) -> Result<Option<T>, JSError> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    read(value, what).map(Some)
}

/// Encodes a command's result for JavaScript through JSON.
fn write<R: Serialize>(value: &R) -> Result<JsValue, JSError> {
    let json = serde_json::to_string(value).map_err(ArgumentError::Unencodable)?;
    js_sys::JSON::parse(&json).map_err(|_| ArgumentError::NotJson { what: "result" }.into())
}

async fn flatten<R>(reply: Reply<Result<R, JSError>>) -> Result<R, JSError> {
    reply.await?
}

fn settle<R: Serialize>(
    future: impl Future<Output = Result<R, JSError>> + 'static,
) -> js_sys::Promise {
    wasm_bindgen_futures::future_to_promise(async move { Ok(write(&future.await?)?) })
}

fn rejected(error: JSError) -> js_sys::Promise {
    js_sys::Promise::reject(&error.into())
}

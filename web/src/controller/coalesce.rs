//! Per-frame command coalescing (`jumpTo`, `setGeoJsonData`) and async `setStyle` resolution.

use std::cell::RefCell;

use futures::channel::oneshot;
use maplibre::{
    io::{
        apc::AsyncProcedureCall,
        source_client::{HttpClient, HttpSourceClient, SourceClient},
        tile_json::resolve_tile_json_sources,
    },
    map::handle::MapHandle,
    render::view_state::CameraOptions,
    style::source::GeoJsonData,
};
use rustc_hash::FxHashMap;
use wasm_bindgen::JsValue;

use super::options::merge_camera;
use crate::{
    application::parse_style,
    error::{ArgumentError, JSError},
    platform::http_client::web_http_client,
};

struct PendingJump {
    camera: CameraOptions,
    waiters: Vec<oneshot::Sender<Result<(), JSError>>>,
}

pub(super) enum PendingGeoJsonPayload {
    Data(GeoJsonData),
    Bytes { bytes: Vec<u8>, was_string: bool },
}

struct PendingGeoJson {
    payload: PendingGeoJsonPayload,
    waiters: Vec<oneshot::Sender<Result<(), JSError>>>,
}

#[derive(Default)]
pub(super) struct CoalescedCommands {
    jump: RefCell<Option<PendingJump>>,
    geojson: RefCell<FxHashMap<String, PendingGeoJson>>,
}

impl CoalescedCommands {
    pub(super) fn drain_waiters(&self) {
        if let Some(pending) = self.jump.borrow_mut().take() {
            for waiter in pending.waiters {
                drop(waiter.send(Ok(())));
            }
        }
        for (_, pending) in self.geojson.borrow_mut().drain() {
            for waiter in pending.waiters {
                drop(waiter.send(Ok(())));
            }
        }
    }
}

pub(super) fn enqueue_jump<E>(
    handle: &MapHandle<E>,
    coalesced: &std::rc::Rc<CoalescedCommands>,
    camera: CameraOptions,
) -> oneshot::Receiver<Result<(), JSError>>
where
    E: maplibre::environment::Environment,
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow:
        maplibre::window::HeadedMapWindow,
{
    let (tx, rx) = oneshot::channel();
    let should_enqueue = {
        let mut slot = coalesced.jump.borrow_mut();
        match slot.as_mut() {
            Some(pending) => {
                pending.camera = merge_camera(pending.camera, camera);
                pending.waiters.push(tx);
                false
            }
            None => {
                *slot = Some(PendingJump {
                    camera,
                    waiters: vec![tx],
                });
                true
            }
        }
    };

    if should_enqueue {
        let coalesced = coalesced.clone();
        drop(handle.call(move |map| {
            if let Some(pending) = coalesced.jump.borrow_mut().take() {
                let err_msg = map.jump_to(pending.camera).err().map(|e| e.to_string());
                for waiter in pending.waiters {
                    let res = match &err_msg {
                        None => Ok(()),
                        Some(msg) => Err(JSError::Web(crate::error::WebError::GenericError(
                            msg.clone().into(),
                        ))),
                    };
                    drop(waiter.send(res));
                }
            }
        }));
    }
    rx
}

pub(super) fn enqueue_geojson<E>(
    handle: &MapHandle<E>,
    coalesced: &std::rc::Rc<CoalescedCommands>,
    source: String,
    payload: PendingGeoJsonPayload,
) -> oneshot::Receiver<Result<(), JSError>>
where
    E: maplibre::environment::Environment,
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow:
        maplibre::window::HeadedMapWindow,
{
    let (tx, rx) = oneshot::channel();
    let should_enqueue = {
        let mut map_slot = coalesced.geojson.borrow_mut();
        match map_slot.get_mut(&source) {
            Some(pending) => {
                pending.payload = payload;
                pending.waiters.push(tx);
                false
            }
            None => {
                map_slot.insert(
                    source.clone(),
                    PendingGeoJson {
                        payload,
                        waiters: vec![tx],
                    },
                );
                true
            }
        }
    };

    if should_enqueue {
        let coalesced = coalesced.clone();
        drop(handle.call(move |map| {
            if let Some(pending) = coalesced.geojson.borrow_mut().remove(&source) {
                let outcome = match pending.payload {
                    PendingGeoJsonPayload::Data(data) => {
                        map.set_geojson_data(&source, data).map_err(JSError::from)
                    }
                    PendingGeoJsonPayload::Bytes { bytes, was_string } => map
                        .set_geojson_bytes_owned(&source, bytes, was_string)
                        .map_err(JSError::from),
                };
                let err_msg = outcome.err().map(|e| e.to_string());
                for waiter in pending.waiters {
                    let res = match &err_msg {
                        None => Ok(()),
                        Some(msg) => Err(JSError::Web(crate::error::WebError::GenericError(
                            msg.clone().into(),
                        ))),
                    };
                    drop(waiter.send(res));
                }
            }
        }));
    }
    rx
}

pub(super) async fn apply_style_swap<E>(handle: MapHandle<E>, style: JsValue) -> Result<(), JSError>
where
    E: maplibre::environment::Environment,
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow:
        maplibre::window::HeadedMapWindow,
{
    let style_json =
        if let Some(text) = style.as_string() {
            let trimmed = text.trim();
            if trimmed.starts_with('{') {
                text
            } else {
                let bytes = web_http_client().fetch(trimmed).await.map_err(|err| {
                    ArgumentError::NotJson {
                        what: if err.is_not_found() {
                            "style URL (404)"
                        } else {
                            "style URL"
                        },
                    }
                })?;
                String::from_utf8(bytes).map_err(|_| ArgumentError::NotJson {
                    what: "style UTF-8",
                })?
            }
        } else {
            js_sys::JSON::stringify(&style)
                .ok()
                .and_then(|json| json.as_string())
                .ok_or(ArgumentError::NotJson { what: "style" })?
        };

    let mut parsed = parse_style(&style_json)?;
    let client = SourceClient::new(HttpSourceClient::new(web_http_client()));
    resolve_tile_json_sources(&mut parsed, &client).await;

    handle
        .call(move |map| {
            if parsed.terrain.is_some()
                || parsed.pitch.is_some_and(|pitch| pitch > 60.0)
                || parsed.projection.as_ref().is_some_and(|p| {
                    p.projection_type != maplibre::projection::ProjectionType::Mercator
                })
            {
                map.set_max_pitch_degrees(parsed.pitch.unwrap_or(0.0).max(70.0));
            }
            drop(map.set_style(parsed));
            map.kernel().apc().set_boot_unthrottled(true);
            if let Ok(ctx) = map.context() {
                map.kernel().apc().warm_style(&ctx.style);
            }
            Ok::<_, JSError>(())
        })
        .await??;
    Ok(())
}

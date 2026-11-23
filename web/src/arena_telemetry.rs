//! Compile-time gated telemetry, camera-sync and idle hooks for external comparison/benchmark
//! harnesses (`--features arena-telemetry`). Absent from default and production builds.

use std::{cell::RefCell, rc::Rc};

use maplibre::{io::apc::AsyncProcedureCall, map::handle::Reply};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::{
    controller::{options::CameraState, with_controller_handle, write_js, MapController},
    environment::WHATWGOffscreenKernelEnvironment,
    error::JSError,
};

/// Active frame-watcher handles attached to a [`MapController`]. Dropping or replacing a [`Reply`]
/// cancels its watcher in [`maplibre::map::handle::MapCommands::observe`] automatically.
#[derive(Clone, Default)]
pub(crate) struct TelemetryWatchers {
    camera: Rc<RefCell<Option<Reply<()>>>>,
    telemetry: Rc<RefCell<Option<Reply<()>>>>,
}

impl TelemetryWatchers {
    pub(crate) fn detach(&self) {
        self.camera.borrow_mut().take();
        self.telemetry.borrow_mut().take();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RedrawSnapshot {
    changed: bool,
    loading: bool,
    awaiting_images: bool,
    camera_moved: bool,
    worker_results: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StageSample {
    name: String,
    ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArenaFrameTelemetry {
    frame: u64,
    host_frame: u64,
    cpu_ms: f64,
    gpu_ms: Option<f64>,
    upload_bytes: u64,
    draws: u32,
    passes: u32,
    set_vertex_buffers: u32,
    write_buffer_calls: u32,
    write_buffer_bytes: u64,
    drape_redraws: u32,
    prefetch_drapes: u32,
    loaded: bool,
    settled: bool,
    wasm_memory_bytes: u64,
    gpu_memory_bytes: u64,
    pending_requests: u32,
    total_requests: u64,
    camera: CameraState,
    redraw_reasons: RedrawSnapshot,
    stages: Vec<StageSample>,
}

fn wasm_linear_memory_bytes() -> u64 {
    use wasm_bindgen::JsCast;
    wasm_bindgen::memory()
        .dyn_into::<js_sys::WebAssembly::Memory>()
        .ok()
        .and_then(|mem| js_sys::Reflect::get(&mem.buffer(), &JsValue::from_str("byteLength")).ok())
        .and_then(|val| val.as_f64())
        .map_or(0, |bytes| bytes.max(0.0) as u64)
}

fn live_gpu_buffer_bytes() -> u64 {
    js_sys::Reflect::get(
        &js_sys::global(),
        &JsValue::from_str("__maplibreLiveGpuBufferBytes"),
    )
    .ok()
    .and_then(|val| val.as_f64())
    .map_or(0, |bytes| bytes.max(0.0) as u64)
}

fn last_gpu_api_stats() -> (u32, u32, u32, u64) {
    use wasm_bindgen::JsCast;
    let Ok(val) = js_sys::Reflect::get(
        &js_sys::global(),
        &JsValue::from_str("__maplibreLastGpuApiStats"),
    ) else {
        return (0, 0, 0, 0);
    };
    let Ok(arr) = val.dyn_into::<js_sys::Float64Array>() else {
        return (0, 0, 0, 0);
    };
    if arr.length() < 4 {
        return (0, 0, 0, 0);
    }
    (
        arr.get_index(0).max(0.0) as u32,
        arr.get_index(1).max(0.0) as u32,
        arr.get_index(2).max(0.0) as u32,
        arr.get_index(3).max(0.0) as u64,
    )
}

#[wasm_bindgen]
impl MapController {
    /// Subscribes to synchronous post-frame camera updates; passing `null`/`undefined` detaches
    /// the watcher so zero per-frame work runs.
    #[wasm_bindgen(js_name = setArenaCameraListener)]
    pub fn set_arena_camera_listener(&self, callback: Option<js_sys::Function>) {
        self.watchers.camera.borrow_mut().take();
        let Some(callback) = callback else {
            return;
        };

        let mut last_rev: Option<u64> = None;
        let reply = with_controller_handle!(&self.handle, |handle| {
            handle.until(move |map| {
                let Ok(context) = map.context() else {
                    return None;
                };
                let rev = context.camera_revision();
                if last_rev != Some(rev) {
                    last_rev = Some(rev);
                    if let Ok(value) = write_js(&CameraState::of(context)) {
                        drop(callback.call1(&JsValue::NULL, &value));
                    }
                }
                None
            })
        });
        *self.watchers.camera.borrow_mut() = Some(reply);
    }

    /// Subscribes to per-frame `FrameStats`, stage timings, GPU pass duration and WASM memory
    /// updates; passing `null`/`undefined` detaches the watcher immediately.
    #[wasm_bindgen(js_name = setArenaTelemetryListener)]
    pub fn set_arena_telemetry_listener(&self, callback: Option<js_sys::Function>) {
        self.watchers.telemetry.borrow_mut().take();
        let Some(callback) = callback else {
            return;
        };

        let mut last_frame: u64 = 0;
        let reply = with_controller_handle!(&self.handle, |handle| {
            handle.until(move |map| {
                let Ok(context) = map.context() else {
                    return None;
                };
                let stats = context.last_frame_stats();
                if stats.frame != 0 && stats.frame != last_frame {
                    last_frame = stats.frame;
                    let has_arrivals = AsyncProcedureCall::<WHATWGOffscreenKernelEnvironment>::has_pending_arrivals(
                        map.kernel().apc(),
                    );
                    let reasons = context.redraw_reasons();
                    let loaded = !reasons.loading
                        && !reasons.awaiting_images
                        && !reasons.camera_moved
                        && !has_arrivals;
                    let (passes, set_vertex_buffers, write_buffer_calls, write_buffer_bytes) =
                        last_gpu_api_stats();
                    let payload = ArenaFrameTelemetry {
                        frame: stats.frame,
                        host_frame: stats.host_frame,
                        cpu_ms: stats.cpu().as_secs_f64() * 1000.0,
                        gpu_ms: stats.gpu.map(|d| d.as_secs_f64() * 1000.0),
                        upload_bytes: stats.upload_bytes,
                        draws: stats.draws,
                        passes,
                        set_vertex_buffers,
                        write_buffer_calls,
                        write_buffer_bytes,
                        drape_redraws: stats.drape_redraws,
                        prefetch_drapes: stats.prefetch_drapes,
                        loaded,
                        settled: !reasons.any()
                            && !has_arrivals
                            && !context.has_pending_drapes()
                            && !context.has_scheduled_retries(),
                        wasm_memory_bytes: wasm_linear_memory_bytes(),
                        gpu_memory_bytes: live_gpu_buffer_bytes(),
                        pending_requests: u32::try_from(
                            maplibre::io::tile_backpressure::tiles_in_flight(&context.world),
                        )
                        .unwrap_or(u32::MAX),
                        total_requests: maplibre::io::tile_backpressure::total_tile_requests(
                            &context.world,
                        ),
                        camera: CameraState::of(context),
                        redraw_reasons: RedrawSnapshot {
                            changed: reasons.changed,
                            loading: reasons.loading,
                            awaiting_images: reasons.awaiting_images,
                            camera_moved: reasons.camera_moved,
                            worker_results: reasons.worker_results || has_arrivals,
                        },
                        stages: stats
                            .stages
                            .iter()
                            .map(|(name, duration)| StageSample {
                                name: name.to_string(),
                                ms: duration.as_secs_f64() * 1000.0,
                            })
                            .collect(),
                    };
                    if let Ok(value) = write_js(&payload) {
                        drop(callback.call1(&JsValue::NULL, &value));
                    }
                }
                None
            })
        });
        *self.watchers.telemetry.borrow_mut() = Some(reply);
    }

    /// Resolves with the camera state once all initial tiles/glyphs/sprites have loaded and at
    /// least one frame has rendered them, without waiting for symbol fade opacity animations.
    #[wasm_bindgen(js_name = waitArenaLoaded)]
    pub fn wait_arena_loaded(&self) -> js_sys::Promise {
        let reply = with_controller_handle!(&self.handle, |handle| {
            handle.until(|map| {
                let has_arrivals =
                    AsyncProcedureCall::<WHATWGOffscreenKernelEnvironment>::has_pending_arrivals(
                        map.kernel().apc(),
                    );
                let context = map.context().ok()?;
                let stats = context.last_frame_stats();
                let reasons = context.redraw_reasons();
                if stats.frame > 0
                    && !reasons.loading
                    && !reasons.awaiting_images
                    && !reasons.camera_moved
                    && !has_arrivals
                {
                    Some(CameraState::of(context))
                } else {
                    None
                }
            })
        });
        wasm_bindgen_futures::future_to_promise(async move {
            let camera = reply.await.map_err(JSError::from)?;
            Ok(write_js(&camera)?)
        })
    }

    /// Resolves with the camera state once the map has rendered at least one frame and has no
    /// pending tile loads, animations, or dirty state (`!needs_redraw()`).
    #[wasm_bindgen(js_name = waitArenaIdle)]
    pub fn wait_arena_idle(&self) -> js_sys::Promise {
        let reply = with_controller_handle!(&self.handle, |handle| {
            handle.until(|map| {
                let has_arrivals =
                    AsyncProcedureCall::<WHATWGOffscreenKernelEnvironment>::has_pending_arrivals(
                        map.kernel().apc(),
                    );
                let context = map.context().ok()?;
                let stats = context.last_frame_stats();
                if stats.frame > 0
                    && !context.needs_redraw()
                    && !context.has_pending_drapes()
                    && !context.has_scheduled_retries()
                    && !has_arrivals
                {
                    Some(CameraState::of(context))
                } else {
                    None
                }
            })
        });
        wasm_bindgen_futures::future_to_promise(async move {
            let camera = reply.await.map_err(JSError::from)?;
            Ok(write_js(&camera)?)
        })
    }
}

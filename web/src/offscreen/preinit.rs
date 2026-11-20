//! Two-phase offscreen WebGPU startup allowing adapter/device init to overlap style fetch.

use std::rc::Rc;

use maplibre::{
    environment::OffscreenKernelConfig,
    kernel::{Kernel, KernelBuilder},
    map::Map,
    render::{builder::RendererBuilder, Renderer},
    style::Style,
    window::{MapWindowConfig, PhysicalSize},
};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, OffscreenCanvas};

use super::{
    start_runner,
    window::{
        next_runner_id, OffscreenCanvasHandle, OffscreenEnvironment, OffscreenMapWindow,
        OffscreenMapWindowConfig, SharedWindowState,
    },
    OffscreenMapRunner,
};
use crate::{
    application::{default_plugins_for, parse_style},
    error::{JSError, WebError},
    platform::{self, http_client::web_http_client},
};

/// WebGPU adapter, device, surface, and worker pool ready to bind to a fetched map style.
#[wasm_bindgen]
pub struct PreinitializedOffscreen {
    scope: DedicatedWorkerGlobalScope,
    size: PhysicalSize,
    scale: f64,
    runner_id: u64,
    window_state: Rc<SharedWindowState>,
    kernel: Kernel<OffscreenEnvironment>,
    renderer: Renderer,
}

struct WindowAndKernel {
    scope: DedicatedWorkerGlobalScope,
    size: PhysicalSize,
    scale: f64,
    runner_id: u64,
    window_state: Rc<SharedWindowState>,
    kernel: Kernel<OffscreenEnvironment>,
}

fn create_window_and_kernel(
    canvas: OffscreenCanvas,
    width: u32,
    height: u32,
    scale_factor: f64,
    new_worker: js_sys::Function,
) -> Result<WindowAndKernel, JSError> {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().dyn_into().map_err(|_| {
        WebError::TypeError("run_maplibre_offscreen requires DedicatedWorkerGlobalScope".into())
    })?;
    let size = PhysicalSize::new(width.max(1), height.max(1)).unwrap_or(PhysicalSize::MIN);
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    canvas.set_width(size.width());
    canvas.set_height(size.height());

    let runner_id = next_runner_id();
    let window_state = Rc::new(SharedWindowState {
        id: runner_id,
        handle: OffscreenCanvasHandle(canvas),
        size: std::cell::Cell::new(size),
        scale_factor: std::cell::Cell::new(scale),
        redraw: std::cell::RefCell::new(None),
    });
    let window_config = OffscreenMapWindowConfig {
        window: OffscreenMapWindow {
            state: window_state.clone(),
        },
    };

    let offscreen_kernel_config = OffscreenKernelConfig {
        cache_directory: None,
        image_providers: platform::image_providers().clone(),
        ..Default::default()
    };
    #[cfg(not(target_feature = "atomics"))]
    let apc = platform::singlethreaded::apc::PassingAsyncProcedureCall::new(
        new_worker,
        6,
        offscreen_kernel_config.clone(),
    )?;
    #[cfg(target_feature = "atomics")]
    let apc = maplibre::io::apc::SchedulerAsyncProcedureCall::new(
        platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler::from_ports(new_worker)?,
        offscreen_kernel_config.clone(),
    );
    let kernel: Kernel<OffscreenEnvironment> = KernelBuilder::new()
        .with_map_window_config(window_config)
        .with_http_client(web_http_client())
        .with_asset_cache(offscreen_kernel_config.asset_cache)
        .with_apc(apc)
        .build()?;

    Ok(WindowAndKernel {
        scope,
        size,
        scale,
        runner_id,
        window_state,
        kernel,
    })
}

fn build_map(
    kernel: Kernel<OffscreenEnvironment>,
    mut style_json: Option<String>,
) -> Result<Map<OffscreenEnvironment>, JSError> {
    let style = match style_json.take().as_deref() {
        Some(raw) => parse_style(raw)?,
        None => Style::default(),
    };
    let plugins = default_plugins_for::<OffscreenEnvironment>(&style);
    let settings = maplibre::render::settings::RendererSettings {
        gpu_timing: cfg!(feature = "arena-telemetry"),
        ..Default::default()
    };
    let mut map = Map::new(
        style.clone(),
        kernel,
        RendererBuilder::new().with_renderer_settings(settings),
        plugins,
    )?;
    if style.terrain.is_some()
        || style.pitch.is_some_and(|pitch| pitch > 60.0)
        || style
            .projection
            .as_ref()
            .is_some_and(|p| p.projection_type != maplibre::projection::ProjectionType::Mercator)
    {
        map.set_max_pitch_degrees(style.pitch.unwrap_or(0.0).max(70.0));
    }
    Ok(map)
}

pub(super) async fn preinit_offscreen_inner(
    canvas: OffscreenCanvas,
    width: u32,
    height: u32,
    scale_factor: f64,
    new_worker: js_sys::Function,
) -> Result<PreinitializedOffscreen, JSError> {
    let wk = create_window_and_kernel(canvas, width, height, scale_factor, new_worker)?;
    let window = wk
        .kernel
        .map_window_config()
        .create()
        .map_err(maplibre::map::MapError::Window)?;
    let settings = maplibre::render::settings::RendererSettings {
        gpu_timing: cfg!(feature = "arena-telemetry"),
        ..Default::default()
    };
    let renderer = RendererBuilder::new()
        .with_renderer_settings(settings)
        .build()
        .initialize_renderer::<OffscreenMapWindowConfig>(&window)
        .await
        .map_err(maplibre::map::MapError::DeviceInit)?;

    Ok(PreinitializedOffscreen {
        scope: wk.scope,
        size: wk.size,
        scale: wk.scale,
        runner_id: wk.runner_id,
        window_state: wk.window_state,
        kernel: wk.kernel,
        renderer,
    })
}

pub(super) async fn finish_offscreen_inner(
    pre: PreinitializedOffscreen,
    style_json: Option<String>,
) -> Result<OffscreenMapRunner, JSError> {
    let mut map = build_map(pre.kernel, style_json)?;
    map.initialize_with_renderer(Some(pre.renderer)).await?;
    if let Ok(context) = map.context_mut() {
        context.resize(pre.size, pre.scale);
    }

    Ok(start_runner(
        pre.runner_id,
        pre.scope,
        pre.window_state,
        map,
    ))
}

/// Initializes WebGPU adapter, device, surface, and tile worker pool before `style_json` arrives.
#[wasm_bindgen]
pub async fn preinit_maplibre_offscreen(
    canvas: OffscreenCanvas,
    width: u32,
    height: u32,
    scale_factor: f64,
    new_worker: js_sys::Function,
) -> Result<PreinitializedOffscreen, JSError> {
    preinit_offscreen_inner(canvas, width, height, scale_factor, new_worker).await
}

/// Binds a preinitialized WebGPU surface and worker pool to `style_json` and starts frame 0.
#[wasm_bindgen]
pub async fn finish_maplibre_offscreen(
    pre: PreinitializedOffscreen,
    style_json: Option<String>,
) -> Result<OffscreenMapRunner, JSError> {
    finish_offscreen_inner(pre, style_json).await
}

/// Initializes and starts a map inside a dedicated Web Worker owning `canvas`.
#[wasm_bindgen]
pub async fn run_maplibre_offscreen(
    canvas: OffscreenCanvas,
    width: u32,
    height: u32,
    scale_factor: f64,
    new_worker: js_sys::Function,
    style_json: Option<String>,
) -> Result<OffscreenMapRunner, JSError> {
    let wk = create_window_and_kernel(canvas, width, height, scale_factor, new_worker)?;
    let mut map = build_map(wk.kernel, style_json)?;
    map.initialize_renderer().await?;
    if let Ok(context) = map.context_mut() {
        context.resize(wk.size, wk.scale);
    }
    Ok(start_runner(wk.runner_id, wk.scope, wk.window_state, map))
}

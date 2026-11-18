//! Window, surface target, and waker types for an `OffscreenCanvas` render worker.

#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use maplibre::{
    environment::Environment,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, PhysicalSize, WindowCreateError},
};
use rustc_hash::FxHashMap;
use web_sys::OffscreenCanvas;

use crate::platform::{http_client::WebHttpClient, UsedOffscreenKernelEnvironment};

#[cfg(not(target_feature = "atomics"))]
pub(super) type WorkerApc = crate::platform::singlethreaded::apc::PassingAsyncProcedureCall;
#[cfg(target_feature = "atomics")]
pub(super) type WorkerApc = maplibre::io::apc::SchedulerAsyncProcedureCall<
    UsedOffscreenKernelEnvironment,
    crate::platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler,
>;

static NEXT_RUNNER_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static RUNNER_WAKERS: RefCell<FxHashMap<u64, Rc<dyn Fn()>>> = RefCell::new(FxHashMap::default());
}

pub(super) fn next_runner_id() -> u64 {
    NEXT_RUNNER_ID.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn register_runner_waker(id: u64, waker: Rc<dyn Fn()>) -> Arc<dyn Fn() + Send + Sync> {
    RUNNER_WAKERS.with(|map| {
        map.borrow_mut().insert(id, waker);
    });
    Arc::new(move || {
        let cb = RUNNER_WAKERS.with(|map| map.borrow().get(&id).cloned());
        if let Some(cb) = cb {
            cb();
        }
    })
}

pub(super) fn unregister_runner_waker(id: u64) {
    RUNNER_WAKERS.with(|map| {
        map.borrow_mut().remove(&id);
    });
}

/// Clonable `OffscreenCanvas` owner convertible into [`wgpu::SurfaceTarget`].
#[derive(Clone)]
pub struct OffscreenCanvasHandle(pub OffscreenCanvas);

impl From<OffscreenCanvasHandle> for wgpu::SurfaceTarget<'static> {
    fn from(handle: OffscreenCanvasHandle) -> Self {
        wgpu::SurfaceTarget::OffscreenCanvas(handle.0)
    }
}

pub(super) struct SharedWindowState {
    pub(super) id: u64,
    pub(super) handle: OffscreenCanvasHandle,
    pub(super) size: Cell<PhysicalSize>,
    pub(super) scale_factor: Cell<f64>,
    pub(super) redraw: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Headed window backed by a worker-owned [`OffscreenCanvas`].
#[derive(Clone)]
pub struct OffscreenMapWindow {
    pub(super) state: Rc<SharedWindowState>,
}

impl MapWindow for OffscreenMapWindow {
    fn size(&self) -> PhysicalSize {
        self.state.size.get()
    }
}

impl HeadedMapWindow for OffscreenMapWindow {
    type WindowHandle = OffscreenCanvasHandle;

    fn handle(&self) -> &Self::WindowHandle {
        &self.state.handle
    }

    fn request_redraw(&self) {
        let cb = self.state.redraw.borrow().clone();
        if let Some(cb) = cb {
            cb();
        }
    }

    fn scale_factor(&self) -> f64 {
        self.state.scale_factor.get()
    }

    fn id(&self) -> u64 {
        self.state.id
    }
}

/// Window configuration creating an [`OffscreenMapWindow`].
#[derive(Clone)]
pub struct OffscreenMapWindowConfig {
    pub(super) window: OffscreenMapWindow,
}

impl MapWindowConfig for OffscreenMapWindowConfig {
    type MapWindow = OffscreenMapWindow;

    fn create(&self) -> Result<Self::MapWindow, WindowCreateError> {
        Ok(self.window.clone())
    }
}

/// Browser worker environment rendering directly into an [`OffscreenCanvas`].
pub struct OffscreenEnvironment;

impl Environment for OffscreenEnvironment {
    type MapWindowConfig = OffscreenMapWindowConfig;
    type AsyncProcedureCall = WorkerApc;
    type HttpClient = WebHttpClient;
    type OffscreenKernelEnvironment = UsedOffscreenKernelEnvironment;
}

//! Dedicated OS worker thread owning a [`HeadlessMap`] and its Tokio runtime.

use std::{
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use cgmath::Deg;
use thiserror::Error;

use super::{SyncHeadlessCommands, SyncHeadlessHandle, SyncWakeHost};
use crate::{
    background::BackgroundPlugin,
    headless::{
        create_headless_renderer_with_settings, environment::HeadlessEnvironment, map::HeadlessMap,
        HeadlessRendererError,
    },
    heatmap::HeatmapPlugin,
    hillshade::HillshadePlugin,
    io::tile_json::resolve_tile_json_sources,
    map::MapError,
    plugin::Plugin,
    render::{settings::RendererSettings, RenderPlugin},
    sdf::SdfPlugin,
    style::Style,
    terrain::{DefaultDemTransferables, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};

/// Configuration for spawning a [`HeadlessRuntimeThread`] around a live [`HeadlessMap`].
#[derive(Clone)]
pub struct HeadlessRuntimeConfig {
    /// Physical width of the offscreen render target in pixels (must be non-zero).
    pub width: u32,
    /// Physical height of the offscreen render target in pixels (must be non-zero).
    pub height: u32,
    /// Physical-to-logical display pixel ratio (`<= 0.0` defaults to `1.0`).
    pub pixel_ratio: f64,
    /// Initial map style document.
    pub style: Style,
    /// Optional filesystem directory for HTTP tile caching.
    pub cache_dir: Option<String>,
    /// Optional maximum camera pitch in degrees (`None` preserves the default limit).
    pub max_pitch_deg: Option<f64>,
    /// Offscreen renderer settings (texture format, MSAA, buffer pool sizes).
    pub renderer_settings: RendererSettings,
    /// Number of Tokio worker threads used for asynchronous tile fetching and decoding.
    pub worker_threads: usize,
}

impl std::fmt::Debug for HeadlessRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessRuntimeConfig")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_ratio", &self.pixel_ratio)
            .field("style", &self.style)
            .field("cache_dir", &self.cache_dir)
            .field("max_pitch_deg", &self.max_pitch_deg)
            .field("worker_threads", &self.worker_threads)
            .finish_non_exhaustive()
    }
}

impl Default for HeadlessRuntimeConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            pixel_ratio: 1.0,
            style: Style::default(),
            cache_dir: None,
            max_pitch_deg: None,
            renderer_settings: RendererSettings::default(),
            worker_threads: 2,
        }
    }
}

/// Why a [`HeadlessRuntimeThread`] could not start.
#[derive(Debug, Error)]
pub enum HeadlessRuntimeError {
    /// Spawning the dedicated map OS thread failed.
    #[error("spawning headless runtime thread failed: {0}")]
    ThreadSpawn(std::io::Error),
    /// Creating the Tokio worker runtime failed.
    #[error("creating headless Tokio runtime failed: {0}")]
    Runtime(std::io::Error),
    /// Offscreen GPU renderer creation failed.
    #[error("headless renderer initialization failed: {0}")]
    Renderer(String),
    /// Initializing [`HeadlessMap`] plugins failed.
    #[error("headless map initialization failed: {0}")]
    Map(String),
    /// Custom map initialization failed.
    #[error("headless map initialization failed: {0}")]
    Custom(String),
    /// The worker thread exited before reporting initialization status.
    #[error("headless runtime thread exited during startup")]
    Aborted,
}

impl From<HeadlessRendererError> for HeadlessRuntimeError {
    fn from(error: HeadlessRendererError) -> Self {
        Self::Renderer(error.to_string())
    }
}

impl From<MapError> for HeadlessRuntimeError {
    fn from(error: MapError) -> Self {
        Self::Map(error.to_string())
    }
}

#[derive(Default)]
struct WorkerState {
    woken: bool,
    shutdown: bool,
}

struct CondvarWake {
    pair: Arc<(Mutex<WorkerState>, Condvar)>,
}

impl SyncWakeHost for CondvarWake {
    fn wake(&self) {
        let (mutex, condvar) = &*self.pair;
        let mut state = match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.woken = true;
        drop(state);
        condvar.notify_one();
    }
}

/// Owns a [`HeadlessMap`] and its multi-threaded Tokio runtime on a dedicated OS thread,
/// exposing a `Clone + Send + Sync` [`SyncHeadlessHandle`] usable from any thread (including
/// JNI threads and C FFI callers).
///
/// Dropping or calling [`Self::shutdown`] wakes the worker thread, closes pending replies with
/// [`super::MapClosed`], drops the map inside its Tokio runtime context, and joins the thread.
pub struct HeadlessRuntimeThread {
    handle: SyncHeadlessHandle,
    signal: Arc<(Mutex<WorkerState>, Condvar)>,
    join_handle: Option<JoinHandle<()>>,
}

impl HeadlessRuntimeThread {
    /// Spawns a dedicated thread that initializes a [`HeadlessMap`] with standard rendering
    /// plugins according to `config`.
    pub fn spawn(config: HeadlessRuntimeConfig) -> Result<Self, HeadlessRuntimeError> {
        let worker_threads = config.worker_threads.max(1);
        Self::spawn_with_threads(worker_threads, move |runtime| {
            let mut style = config.style;
            let (kernel, renderer) = runtime.block_on(create_headless_renderer_with_settings(
                config.width,
                config.height,
                config.cache_dir,
                config.renderer_settings,
            ))?;
            runtime.block_on(resolve_tile_json_sources(
                &mut style,
                kernel.source_client(),
            ));
            let plugins = standard_headless_plugins();
            let mut map = HeadlessMap::new(style, renderer, kernel, plugins)?;
            if config.pixel_ratio.is_finite() && config.pixel_ratio > 0.0 {
                map.set_pixel_ratio(config.pixel_ratio);
            }
            if let Some(max_pitch) = config.max_pitch_deg.filter(|deg| deg.is_finite()) {
                map.set_max_pitch(Deg(max_pitch.clamp(0.0, 89.5)));
            }
            Ok(map)
        })
    }

    /// Spawns a dedicated thread running a custom `build` closure inside a fresh Tokio runtime.
    pub fn spawn_with(
        build: impl FnOnce(&tokio::runtime::Runtime) -> Result<HeadlessMap, HeadlessRuntimeError>
            + Send
            + 'static,
    ) -> Result<Self, HeadlessRuntimeError> {
        Self::spawn_with_threads(2, build)
    }

    fn spawn_with_threads(
        worker_threads: usize,
        build: impl FnOnce(&tokio::runtime::Runtime) -> Result<HeadlessMap, HeadlessRuntimeError>
            + Send
            + 'static,
    ) -> Result<Self, HeadlessRuntimeError> {
        let signal = Arc::new((Mutex::new(WorkerState::default()), Condvar::new()));
        let worker_signal = Arc::clone(&signal);
        let (startup_tx, startup_rx) =
            mpsc::sync_channel::<Result<SyncHeadlessHandle, HeadlessRuntimeError>>(1);

        let join_handle = thread::Builder::new()
            .name("maplibre-headless".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(worker_threads.max(1))
                    .thread_name("maplibre-tile-worker")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = startup_tx.send(Err(HeadlessRuntimeError::Runtime(err)));
                        return;
                    }
                };
                let _enter = runtime.enter();
                let mut map = match build(&runtime) {
                    Ok(map) => map,
                    Err(err) => {
                        let _ = startup_tx.send(Err(err));
                        return;
                    }
                };

                let commands = SyncHeadlessCommands::new();
                let handle = commands.handle(CondvarWake {
                    pair: Arc::clone(&worker_signal),
                });
                if startup_tx.send(Ok(handle)).is_err() {
                    commands.close();
                    drop(map);
                    return;
                }

                run_worker_loop(&mut map, &commands, &worker_signal);
                commands.close();
                drop(map);
            })
            .map_err(HeadlessRuntimeError::ThreadSpawn)?;

        match startup_rx.recv() {
            Ok(Ok(handle)) => Ok(Self {
                handle,
                signal,
                join_handle: Some(join_handle),
            }),
            Ok(Err(err)) => {
                let _ = join_handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = join_handle.join();
                Err(HeadlessRuntimeError::Aborted)
            }
        }
    }

    /// Returns a `Clone + Send + Sync` handle to enqueue commands and watchers on this map.
    pub fn handle(&self) -> SyncHeadlessHandle {
        self.handle.clone()
    }

    /// Shuts down the dedicated map thread and waits for it to exit cleanly.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        {
            let (mutex, condvar) = &*self.signal;
            let mut state = match mutex.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.shutdown = true;
            state.woken = true;
            drop(state);
            condvar.notify_all();
        }
        if let Some(join_handle) = self.join_handle.take() {
            if join_handle.thread().id() != thread::current().id() {
                let _ = join_handle.join();
            }
        }
    }
}

impl Drop for HeadlessRuntimeThread {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn run_worker_loop(
    map: &mut HeadlessMap,
    commands: &SyncHeadlessCommands,
    signal: &(Mutex<WorkerState>, Condvar),
) {
    const WATCHER_POLL_INTERVAL: Duration = Duration::from_millis(8);
    const FRAME_STEP: Duration = Duration::from_millis(16);

    loop {
        {
            let (mutex, _) = signal;
            let mut state = match mutex.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if state.shutdown {
                return;
            }
            state.woken = false;
        }

        commands.apply(map);

        if commands.has_watchers() && (map.has_ready_work() || map.needs_redraw()) {
            map.frame_input_mut().advance(FRAME_STEP);
            let _ = map.run_frame();
            commands.observe(map);
        }

        if commands.has_pending_commands() {
            continue;
        }

        let (mutex, condvar) = signal;
        let state = match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.shutdown {
            return;
        }
        if state.woken {
            continue;
        }
        if commands.has_watchers() {
            if map.has_ready_work() {
                continue;
            }
            drop(condvar.wait_timeout(state, WATCHER_POLL_INTERVAL));
        } else {
            drop(condvar.wait(state));
        }
    }
}

fn standard_headless_plugins() -> Vec<Box<dyn Plugin<HeadlessEnvironment>>> {
    vec![
        Box::new(RenderPlugin),
        #[cfg(feature = "experimental")]
        Box::new(crate::starfield::StarfieldPlugin),
        Box::new(BackgroundPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        #[cfg(feature = "raster")]
        Box::new(crate::raster::RasterPlugin::<
            crate::raster::DefaultRasterTransferables,
        >::default()),
        Box::new(HillshadePlugin),
        Box::new(HeatmapPlugin),
        Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
    ]
}

//! Safe Rust offscreen map engine shared by the C ABI and mobile SDK bindings.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use cgmath::Deg;
use maplibre::{
    headless::map::{
        readback::{FrameReadback, PixelLayout},
        HeadlessMap,
    },
    map::handle::{HeadlessRuntimeConfig, HeadlessRuntimeThread, MapClosed, SyncHeadlessHandle},
    sdf::query::{QueryGeometry, QueryOptions},
    style::{feature_state::FeatureId, StyleImage},
    window::PhysicalSize,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::types::{
    apply_whole_style, build_feature_state_target, compact_renderer_settings,
    parse_feature_state_args, parse_style_json, CameraState, FfiError, MapEngineConfig,
};

#[derive(Default, Deserialize)]
struct QueryOptionsWire {
    #[serde(default)]
    layers: Option<Vec<String>>,
    #[serde(default)]
    filter: Option<Value>,
}

fn pack_size(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn unpack_size(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Owns a [`HeadlessRuntimeThread`] and tracks physical render target dimensions atomically.
pub struct MapEngine {
    runtime: HeadlessRuntimeThread,
    packed_size: Arc<AtomicU64>,
}

impl MapEngine {
    /// Creates a new offscreen map engine on a dedicated worker thread.
    pub fn create(config: MapEngineConfig, style_json: &str) -> Result<Self, FfiError> {
        if config.width == 0 || config.height == 0 {
            return Err(FfiError::InvalidSize {
                width: config.width,
                height: config.height,
            });
        }
        if !config.pixel_ratio.is_finite() {
            return Err(FfiError::InvalidNumber("pixel_ratio"));
        }
        if !config.max_pitch_deg.is_finite() {
            return Err(FfiError::InvalidNumber("max_pitch_deg"));
        }
        let style = parse_style_json(style_json)?;
        let pixel_ratio = if config.pixel_ratio > 0.0 {
            config.pixel_ratio
        } else {
            1.0
        };
        let max_pitch_deg = (config.max_pitch_deg > 0.0).then_some(config.max_pitch_deg);
        let runtime = HeadlessRuntimeThread::spawn(HeadlessRuntimeConfig {
            width: config.width,
            height: config.height,
            pixel_ratio,
            style,
            cache_dir: config.cache_dir.filter(|dir| !dir.is_empty()),
            max_pitch_deg,
            renderer_settings: compact_renderer_settings(),
            worker_threads: 2,
        })?;
        Ok(Self {
            runtime,
            packed_size: Arc::new(AtomicU64::new(pack_size(config.width, config.height))),
        })
    }

    /// Current `(width, height)` in physical pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        unpack_size(self.packed_size.load(Ordering::Acquire))
    }

    /// Clones a thread-safe [`SyncHeadlessHandle`] for asynchronous commands.
    pub fn clone_handle(&self) -> Result<SyncHeadlessHandle, FfiError> {
        let handle = self.runtime.handle();
        if handle.is_closed() {
            return Err(FfiError::Closed(MapClosed));
        }
        Ok(handle)
    }

    /// Resizes the render target and updates `pixel_ratio` (`<= 0.0` defaults to `1.0`).
    pub fn resize(&self, width: u32, height: u32, pixel_ratio: f64) -> Result<(), FfiError> {
        let Some(size) = PhysicalSize::new(width, height) else {
            return Err(FfiError::InvalidSize { width, height });
        };
        if !pixel_ratio.is_finite() {
            return Err(FfiError::InvalidNumber("pixel_ratio"));
        }
        let ratio = if pixel_ratio > 0.0 { pixel_ratio } else { 1.0 };
        let packed = Arc::clone(&self.packed_size);
        self.runtime.handle().call_blocking(move |map| {
            map.set_pixel_ratio(ratio);
            map.resize(size);
            packed.store(pack_size(width, height), Ordering::Release);
        })?;
        Ok(())
    }

    /// Reads the current camera state.
    pub fn camera(&self) -> Result<CameraState, FfiError> {
        Ok(self.runtime.handle().call_blocking(|map| {
            let view = map.view_state();
            let center = view.center_lat_lon();
            let cam = view.camera();
            CameraState {
                latitude: center.latitude,
                longitude: center.longitude,
                zoom: view.zoom().value(),
                bearing_deg: Deg::from(cam.get_bearing()).0,
                pitch_deg: Deg::from(cam.get_pitch()).0,
            }
        })?)
    }

    /// Jumps the camera immediately to `camera`.
    pub fn set_camera(&self, camera: CameraState) -> Result<(), FfiError> {
        let options = camera.to_options()?;
        self.runtime
            .handle()
            .call_blocking(move |map| map.jump_to(options).map_err(FfiError::from))??;
        Ok(())
    }

    /// Replaces the entire style document from JSON while retaining the camera.
    pub fn set_style_json(&self, style_json: &str) -> Result<(), FfiError> {
        let style = parse_style_json(style_json)?;
        self.runtime
            .handle()
            .call_blocking(move |map| apply_whole_style(map, style))??;
        Ok(())
    }

    /// Sets a layer paint property from JSON (`"null"` restores default).
    pub fn set_paint_property_json(
        &self,
        layer_id: &str,
        property_name: &str,
        value_json: &str,
    ) -> Result<(), FfiError> {
        let value: Value = serde_json::from_str(value_json).map_err(|source| FfiError::Json {
            context: "value_json",
            source,
        })?;
        let layer = layer_id.to_owned();
        let name = property_name.to_owned();
        self.runtime.handle().call_blocking(move |map| {
            map.mutate_style(|style| style.set_paint_property(&layer, &name, value))
                .map(|_| ())
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Sets a layer layout property from JSON (`"null"` restores default).
    pub fn set_layout_property_json(
        &self,
        layer_id: &str,
        property_name: &str,
        value_json: &str,
    ) -> Result<(), FfiError> {
        let value: Value = serde_json::from_str(value_json).map_err(|source| FfiError::Json {
            context: "value_json",
            source,
        })?;
        let layer = layer_id.to_owned();
        let name = property_name.to_owned();
        self.runtime.handle().call_blocking(move |map| {
            map.mutate_style(|style| style.set_layout_property(&layer, &name, value))
                .map(|_| ())
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Sets or removes (`None` or `"null"`) a layer filter expression.
    pub fn set_filter_json(
        &self,
        layer_id: &str,
        filter_json: Option<&str>,
    ) -> Result<(), FfiError> {
        let filter = match filter_json {
            None => None,
            Some(raw) => {
                let parsed: Value = serde_json::from_str(raw).map_err(|source| FfiError::Json {
                    context: "filter_json",
                    source,
                })?;
                if parsed.is_null() {
                    None
                } else {
                    Some(parsed)
                }
            }
        };
        let layer = layer_id.to_owned();
        self.runtime.handle().call_blocking(move |map| {
            map.mutate_style(|style| style.set_filter(&layer, filter))
                .map(|_| ())
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Sets a layer's `visibility` layout property.
    pub fn set_layer_visibility(&self, layer_id: &str, visible: bool) -> Result<(), FfiError> {
        let layer = layer_id.to_owned();
        let value = json!(if visible { "visible" } else { "none" });
        self.runtime.handle().call_blocking(move |map| {
            map.mutate_style(|style| style.set_layout_property(&layer, "visibility", value))
                .map(|_| ())
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Replaces the inline GeoJSON document of `source_id`.
    pub fn set_geojson_source_data(
        &self,
        source_id: &str,
        geojson_json: &str,
    ) -> Result<(), FfiError> {
        let document: Value =
            serde_json::from_str(geojson_json).map_err(|source| FfiError::Json {
                context: "geojson_json",
                source,
            })?;
        let source_name = source_id.to_owned();
        self.runtime.handle().call_blocking(move |map| {
            map.set_geojson_source(&source_name, document)
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Merges JSON object `state_json` into `feature_id`'s runtime `feature-state`.
    pub fn set_feature_state_json(
        &self,
        source: &str,
        source_layer: Option<&str>,
        feature_id: &str,
        state_json: &str,
    ) -> Result<(), FfiError> {
        let (target, id, state_map) =
            parse_feature_state_args(source, source_layer, feature_id, state_json)?;
        self.runtime.handle().call_blocking(move |map| {
            map.set_feature_state(target, id, state_map);
        })?;
        Ok(())
    }

    /// Removes runtime `feature-state` keys/features in `source` / optional `source_layer`.
    pub fn remove_feature_state(
        &self,
        source: &str,
        source_layer: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) -> Result<(), FfiError> {
        let target = build_feature_state_target(source, source_layer);
        let id = feature_id.map(FeatureId::from);
        let key = state_key.map(str::to_owned);
        self.runtime.handle().call_blocking(move |map| {
            map.remove_feature_state(&target, id, key.as_deref());
        })?;
        Ok(())
    }

    /// Adds or replaces a named sprite/icon image from straight-alpha RGBA8 pixels.
    pub fn add_image_rgba8(
        &self,
        image_id: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        pixel_ratio: f32,
        sdf: bool,
    ) -> Result<(), FfiError> {
        if width == 0 || height == 0 {
            return Err(FfiError::InvalidSize { width, height });
        }
        if !pixel_ratio.is_finite() {
            return Err(FfiError::InvalidNumber("pixel_ratio"));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(FfiError::InvalidSize { width, height })?;
        if rgba.len() != expected {
            return Err(FfiError::InvalidBufferLength {
                expected,
                actual: rgba.len(),
            });
        }
        let ratio = if pixel_ratio > 0.0 { pixel_ratio } else { 1.0 };
        let id = image_id.to_owned();
        let image = StyleImage {
            width,
            height,
            data: rgba.to_vec(),
            pixel_ratio: ratio,
            sdf,
        };
        self.runtime.handle().call_blocking(move |map| {
            map.mutate_style(|style| Ok(style.add_image(&id, image)))
                .map(|_| ())
                .map_err(FfiError::from)
        })??;
        Ok(())
    }

    /// Whether the map currently needs another frame.
    pub fn needs_redraw(&self) -> Result<bool, FfiError> {
        Ok(self
            .runtime
            .handle()
            .call_blocking(|map| map.needs_redraw())?)
    }

    /// Renders a live/settled RGBA8 frame and returns its tightly packed bytes (`width * height * 4`).
    pub fn render_rgba8_vec(&self) -> Result<Vec<u8>, FfiError> {
        let (width, height) = self.dimensions();
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(FfiError::InvalidSize { width, height })?;
        self.runtime.handle().call_blocking(move |map| {
            step_settled_or_live_frame(map)?;
            let texture = map.head_texture().ok_or(FfiError::MissingTargetTexture)?;
            let (tex_w, tex_h) = (texture.width(), texture.height());
            let mut readback = FrameReadback::new(map, tex_w, tex_h);
            let mut pixels = vec![0_u8; expected];
            readback.read_into_blocking(map, texture, PixelLayout::Rgba8, &mut pixels)?;
            Ok(pixels)
        })?
    }

    /// Queries rendered features in `[min_x, min_y, max_x, max_y]` and serializes them to JSON.
    pub fn query_rendered_features_json(
        &self,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
        options_json: Option<&str>,
    ) -> Result<String, FfiError> {
        if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
            return Err(FfiError::InvalidNumber("query_geometry"));
        }
        let geometry = if (min_x - max_x).abs() == 0.0 && (min_y - max_y).abs() == 0.0 {
            QueryGeometry::Point([min_x, min_y])
        } else {
            QueryGeometry::Box {
                min: [min_x, min_y],
                max: [max_x, max_y],
            }
        };
        let options = match options_json.filter(|raw| !raw.trim().is_empty()) {
            None => QueryOptions::default(),
            Some(raw) => {
                let wire: QueryOptionsWire =
                    serde_json::from_str(raw).map_err(|source| FfiError::Json {
                        context: "options_json",
                        source,
                    })?;
                QueryOptions {
                    layers: wire.layers,
                    filter: wire.filter,
                }
            }
        };
        self.runtime.handle().call_blocking(move |map| {
            let features = map.query_rendered_features(geometry, &options)?;
            serde_json::to_string(&features).map_err(|source| FfiError::Json {
                context: "queried_features",
                source,
            })
        })?
    }
}

fn step_settled_or_live_frame(map: &mut HeadlessMap) -> Result<(), FfiError> {
    const MAX_SETTLE_FRAMES: usize = 16;
    const SETTLE_BUDGET: Duration = Duration::from_millis(200);
    const POLL_SLEEP: Duration = Duration::from_millis(2);
    const FRAME_DT: Duration = Duration::from_millis(16);

    let deadline = Instant::now() + SETTLE_BUDGET;
    let mut frames_run = 0;

    loop {
        map.frame_input_mut().advance(FRAME_DT);
        map.run_frame()?;
        frames_run += 1;

        if !map.needs_redraw() || frames_run >= MAX_SETTLE_FRAMES {
            break;
        }
        if map.has_ready_work() {
            continue;
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(POLL_SLEEP);
        if !map.has_ready_work() && Instant::now() >= deadline {
            break;
        }
    }
    Ok(())
}

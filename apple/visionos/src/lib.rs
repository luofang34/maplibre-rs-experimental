//! C ABI of the maplibre-rs renderer for the visionOS host app.
//!
//! The host owns the compositor. Per frame it says where the map's scene stands in its world
//! and, per eye, where the eye is and what it sees. The map draws the eye into its own Metal
//! texture and returns it for the host to copy into the compositor's drawable, and writes
//! the eye's depth straight into the compositor's depth texture.

// Crossing the C boundary needs raw pointers; every unsafe block below is that crossing.
#![allow(unsafe_code)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    ffi::{c_char, c_void, CStr},
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    ptr,
    sync::{Arc, Mutex, Once},
    time::Duration,
};

use cgmath::{Deg, Matrix4};
use maplibre::{
    coords::LatLon,
    tcs::system::heap,
    headless::{create_headless_renderer_with_settings, map::HeadlessMap},
    io::tile_json::resolve_tile_json_sources,
    plugin::Plugin,
    render::{
        camera::EyeFrustum,
        settings::{BufferPoolSizes, RendererSettings},
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};
use metal::foreign_types::{ForeignType, ForeignTypeRef};
use tracing_subscriber::{
    filter::Targets, fmt::MakeWriter, layer::SubscriberExt, util::SubscriberInitExt, Layer,
};

/// The format the map renders in: the linear twin of the compositor's `bgra8Unorm_srgb`, so
/// a frame can be drawn straight into a linear view of the drawable, or copied into it.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// A live map rendering into an offscreen texture.
pub struct MaplibreVisionOSMap {
    runtime: tokio::runtime::Runtime,
    map: HeadlessMap,
    width: u32,
    height: u32,
    frames: u64,
}

/// Where the map's scene stands in the host's world. Mirrors `MaplibreVisionOSPlacement` in
/// the header.
#[repr(C)]
pub struct MaplibreVisionOSPlacement {
    /// Latitude of the scene origin in degrees.
    pub anchor_latitude: f64,
    /// Longitude of the scene origin in degrees.
    pub anchor_longitude: f64,
    /// Altitude of the scene origin in metres above sea level.
    pub anchor_altitude_meters: f64,
    /// Column-major 4x4 matrix from the scene, metres east, north and up from the anchor, to
    /// the host's world; a rotation, a translation and one uniform scale.
    pub world_from_scene: *const f32,
}

/// One eye of a frame. Mirrors `MaplibreVisionOSEye` in the header.
#[repr(C)]
pub struct MaplibreVisionOSEye {
    /// Column-major 4x4 matrix from eye space, x right, y up, looking along negative z, to
    /// the host's world.
    pub world_from_eye: *const f32,
    /// Tangents of the frustum's left, right, top and bottom half angles.
    pub tangents: *const f32,
    /// Near plane in the host's metres.
    pub near: f32,
    /// Far plane in the host's metres; a value that is not finite or not beyond the near
    /// plane is replaced by one far enough for the whole globe.
    pub far: f32,
    /// `id<MTLTexture>` of a `bgra8Unorm` or `bgra8Unorm_srgb` texture with the
    /// `pixelFormatView` usage the eye is drawn into, or null to draw into the map's own
    /// texture.
    pub color_texture: *const c_void,
    /// `id<MTLTexture>` of a `Depth32Float` texture the eye's depth is written to, or null.
    pub depth_texture: *const c_void,
}

static LOGGING: Once = Once::new();

/// The system allocator with a running count of live bytes, so the renderer can charge a
/// frame's heap growth to the system it happens in.
struct CountingAllocator;

// SAFETY: every call forwards to the system allocator unchanged; only a counter is kept.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            heap::account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        heap::account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            heap::account(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Name of the log file written next to the tile cache; a console cannot always be
/// attached to the device, and an abort leaves nothing else behind.
const LOG_FILE: &str = "maplibre.log";
/// Name of the file a panic is appended to before the process aborts.
const PANIC_FILE: &str = "panic.log";

/// Writes every log line to standard output and, when a directory is given, to a file in it.
/// Neither write may fail: the subscriber reports a failed write on standard error, and the
/// process aborts when that fails too, which it does once the console a launcher attached
/// has gone away. Standard output and error are best effort here.
struct Tee(Option<Arc<Mutex<File>>>);

struct TeeWriter(Option<Arc<Mutex<File>>>);

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> TeeWriter {
        TeeWriter(self.0.clone())
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::stdout().write_all(buf).ok();
        if let Some(file) = &self.0 {
            if let Ok(mut file) = file.lock() {
                file.write_all(buf).ok();
                file.flush().ok();
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush().ok();
        Ok(())
    }
}

fn init_logging(directory: Option<&str>) {
    LOGGING.call_once(|| {
        let file = directory
            .and_then(|directory| File::create(Path::new(directory).join(LOG_FILE)).ok())
            .map(|file| Arc::new(Mutex::new(file)));
        // wgpu reports every wait on a submission at INFO, ten lines a frame.
        let targets = Targets::new()
            .with_default(tracing::Level::INFO)
            .with_target("wgpu_core", tracing::Level::WARN)
            .with_target("wgpu_hal", tracing::Level::WARN)
            .with_target("naga", tracing::Level::WARN);
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(Tee(file))
                    .with_filter(targets),
            )
            .init();
        if let Some(directory) = directory {
            let panic_path = Path::new(directory).join(PANIC_FILE);
            std::panic::set_hook(Box::new(move |info| {
                let backtrace = std::backtrace::Backtrace::force_capture();
                tracing::error!(%info, "panic");
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&panic_path)
                {
                    file.write_all(format!("{info}\n{backtrace}\n").as_bytes())
                        .ok();
                }
            }));
        }
    });
}

/// Reads a C string argument; `None` for a null pointer or invalid UTF-8.
unsafe fn c_str<'a>(pointer: *const c_char) -> Option<&'a str> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller passes a NUL-terminated string that outlives the call.
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

/// Tells the map how many bytes the process may still take before the system would kill
/// it, as `os_proc_available_memory` reports; below a reserve the map takes nothing new
/// on. Zero means the host cannot tell.
///
/// # Safety
///
/// `map` must be a map created by this library, or null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_available_memory(
    map: *mut MaplibreVisionOSMap,
    available_bytes: u64,
) {
    if map.is_null() {
        return;
    }
    // SAFETY: the caller upholds the map contract documented above.
    let map = unsafe { &mut *map };
    map.map
        .set_available_memory((available_bytes > 0).then_some(available_bytes));
}

/// Writes a line of the host's into the renderer's log, so a host's frame and memory
/// figures sit next to the renderer's own in the file a device keeps.
///
/// # Safety
///
/// `message` must be a NUL-terminated string or null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_note(message: *const c_char) {
    // SAFETY: the caller upholds the string contract documented above.
    if let Some(message) = unsafe { c_str(message) } {
        tracing::info!(target: "host", "{message}");
    }
}

/// Version of the renderer library, as a static C string.
#[no_mangle]
pub extern "C" fn maplibre_visionos_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Creates a map of `width` by `height` pixels from a style JSON, with an optional tile cache
/// directory. Returns null when the style or the renderer cannot be set up.
///
/// # Safety
///
/// `style_json` must be a NUL-terminated string; `cache_dir` may be null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_create(
    style_json: *const c_char,
    width: u32,
    height: u32,
    cache_dir: *const c_char,
) -> *mut MaplibreVisionOSMap {
    // SAFETY: the caller upholds the string contract documented above.
    let (style_json, cache_dir) = unsafe { (c_str(style_json), c_str(cache_dir)) };
    init_logging(cache_dir);
    tracing::info!(width, height, cache_dir = ?cache_dir, "creating the map");
    let Some(style_json) = style_json else {
        tracing::error!("maplibre_visionos_create needs a style");
        return ptr::null_mut();
    };
    match create(style_json, width, height, cache_dir.map(str::to_string)) {
        Ok(map) => Box::into_raw(Box::new(map)),
        Err(error) => {
            tracing::error!(%error, "cannot create the map");
            ptr::null_mut()
        }
    }
}

fn create(
    style_json: &str,
    width: u32,
    height: u32,
    cache_dir: Option<String>,
) -> Result<MaplibreVisionOSMap, String> {
    let mut style: Style =
        serde_json::from_str(style_json).map_err(|error| format!("style: {error}"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime: {error}"))?;
    // The desktop pool sizes hold close to a gigabyte on the GPU before a tile is drawn.
    // These hold a few hundred drawn tiles within the device's memory limit; a pool that
    // cannot hold the drawn tiles has the upload system reload them every frame.
    let settings = RendererSettings {
        texture_format: Some(SURFACE_FORMAT),
        // The upload writes one feature entry per vertex, so the feature ring holds as many
        // entries as the vertex ring holds vertices; a smaller one would evict every frame.
        // Fills index each vertex about three times.
        buffer_pools: BufferPoolSizes {
            vertices: 8_000_000,
            indices: 24_000_000,
            feature_metadata: 8_000_000,
            layer_metadata: 10 * 1024,
        },
        symbol_pools: BufferPoolSizes {
            vertices: 2_000_000,
            indices: 4_000_000,
            feature_metadata: 2_000_000,
            layer_metadata: 10 * 1024,
        },
        ..RendererSettings::default()
    };
    let (kernel, renderer) = runtime
        .block_on(create_headless_renderer_with_settings(
            width, height, cache_dir, settings,
        ))
        .map_err(|error| format!("renderer: {error}"))?;
    runtime.block_on(resolve_tile_json_sources(
        &mut style,
        kernel.source_client(),
    ));
    let has_terrain = style.terrain.is_some();
    let mut plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(maplibre::background::BackgroundPlugin),
        Box::new(maplibre::vector::VectorPlugin::<
            maplibre::vector::DefaultVectorTransferables,
        >::default()),
        Box::new(maplibre::sdf::SdfPlugin::<
            maplibre::vector::DefaultVectorTransferables,
        >::default()),
        Box::new(maplibre::raster::RasterPlugin::<
            maplibre::raster::DefaultRasterTransferables,
        >::default()),
        Box::new(maplibre::hillshade::HillshadePlugin),
    ];
    if has_terrain {
        plugins.push(Box::new(maplibre::terrain::TerrainPlugin::<
            maplibre::terrain::DefaultDemTransferables,
        >::default()));
    }
    let map = {
        let _runtime = runtime.enter();
        let mut map = HeadlessMap::new(style, renderer, kernel, plugins)
            .map_err(|error| format!("map: {error:?}"))?;
        map.set_max_pitch(Deg(89.0));
        map
    };
    tracing::info!(width, height, has_terrain, "map created");
    Ok(MaplibreVisionOSMap {
        runtime,
        map,
        width,
        height,
        frames: 0,
    })
}

/// The Metal command queue (`id<MTLCommandQueue>`) the map submits its draws on, owned by the
/// map and valid while it lives. Work a host commits on it runs after the map's draws, so a
/// copy out of the map's texture needs no wait on the CPU.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_command_queue(
    map: *const MaplibreVisionOSMap,
) -> *const c_void {
    // SAFETY: the caller upholds the handle contract documented above.
    let Some(handle) = (unsafe { map.as_ref() }) else {
        return ptr::null();
    };
    // SAFETY: the renderer runs on Metal on this platform; the queue object stays alive with
    // the map, which holds it. `raw_handle` comes from the vendored wgpu-hal, see
    // vendor-wgpu.sh.
    unsafe {
        handle
            .map
            .queue()
            .as_hal::<wgpu_hal::api::Metal, _, _>(|queue| {
                queue.map_or(ptr::null(), |queue| queue.raw_handle().as_ptr().cast())
            })
    }
    .unwrap_or(ptr::null())
}

/// Far clip distance as a multiple of the near one when the compositor gives none: a tenth
/// of a metre near puts the far plane a million kilometres out, where the gap between the
/// flat map's edge and the horizon is a hundredth of a pixel from any height a viewer stands.
const FAR_OVER_NEAR: f64 = 1.0e10;

/// Terrain elevation in metres at a location, from the DEM tiles the map has loaded; NaN
/// without terrain or before a tile covering the location arrived. A host anchors the scene
/// on the terrain with it, so a height above the anchor is a height above the ground.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_terrain_elevation(
    map: *const MaplibreVisionOSMap,
    latitude: f64,
    longitude: f64,
) -> f32 {
    // SAFETY: the caller upholds the handle contract documented above.
    unsafe { map.as_ref() }
        .and_then(|handle| {
            handle
                .map
                .terrain_elevation_at(LatLon::new(latitude, longitude))
        })
        .map_or(f32::NAN, |elevation| elevation as f32)
}

/// Width in pixels of the texture the map renders into.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_width(map: *const MaplibreVisionOSMap) -> u32 {
    // SAFETY: the caller upholds the handle contract documented above.
    unsafe { map.as_ref() }.map_or(0, |map| map.width)
}

/// Height in pixels of the texture the map renders into.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_height(map: *const MaplibreVisionOSMap) -> u32 {
    // SAFETY: the caller upholds the handle contract documented above.
    unsafe { map.as_ref() }.map_or(0, |map| map.height)
}

/// Draws every eye of a frame and returns the Metal texture (`id<MTLTexture>`) the map draws
/// into when an eye has no colour texture of its own, owned by the map and valid until the
/// next call; null when the frame could not be drawn. Depth textures are written before this
/// returns.
///
/// `request_overscan` widens each eye's frustum for tile requests, one for none.
/// `timestamp_seconds` drives animations and must not decrease.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`]; `placement` must point at a struct
/// whose matrix pointer addresses 16 floats; `eyes` must point at `eye_count` structs whose
/// matrix pointers address 16 floats and tangent pointers 4, all alive for the call; the
/// textures, if any, must be of the map's size and stay alive for the call.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_render_frame(
    map: *mut MaplibreVisionOSMap,
    placement: *const MaplibreVisionOSPlacement,
    prefetch: *const MaplibreVisionOSPlacement,
    eyes: *const MaplibreVisionOSEye,
    eye_count: u32,
    request_overscan: f32,
    timestamp_seconds: f64,
) -> *const c_void {
    // SAFETY: the caller upholds the pointer contracts documented above.
    let (Some(handle), Some(placement)) = (unsafe { map.as_mut() }, unsafe { placement.as_ref() })
    else {
        return ptr::null();
    };
    if placement.world_from_scene.is_null() || eyes.is_null() || eye_count == 0 {
        return ptr::null();
    }
    // SAFETY: as above, the placement matrix holds 16 floats and `eyes` holds `eye_count` eyes.
    let (world_from_scene, eyes) = unsafe {
        (
            std::slice::from_raw_parts(placement.world_from_scene, 16),
            std::slice::from_raw_parts(eyes, eye_count as usize),
        )
    };
    let mut frame_eyes = Vec::with_capacity(eyes.len());
    for eye in eyes {
        if eye.world_from_eye.is_null() || eye.tangents.is_null() {
            return ptr::null();
        }
        // SAFETY: as above, the matrix holds 16 floats and the tangents 4.
        let (world_from_eye, tangents) = unsafe {
            (
                std::slice::from_raw_parts(eye.world_from_eye, 16),
                std::slice::from_raw_parts(eye.tangents, 4),
            )
        };
        let near = f64::from(eye.near);
        // A compositor that reprojects reports no far plane. The flat map's plane reaches the
        // far plane, and the sky starts at the horizon, so the far plane sits close enough to
        // the horizon that the gap between the two stays under a pixel.
        let far = if eye.far.is_finite() && eye.far > eye.near {
            f64::from(eye.far)
        } else {
            near * FAR_OVER_NEAR
        };
        // SAFETY: the caller keeps the textures alive for the call.
        let (color, depth) = unsafe {
            (
                import_color_texture(handle.map.device(), eye.color_texture),
                import_depth_texture(handle.map.device(), eye.depth_texture),
            )
        };
        frame_eyes.push(XrEye {
            world_from_eye: column_major(world_from_eye),
            frustum: EyeFrustum {
                left: f64::from(tangents[0]),
                right: f64::from(tangents[1]),
                top: f64::from(tangents[2]),
                bottom: f64::from(tangents[3]),
                near,
                far,
            },
            target: EyeTarget { color, depth },
        });
    }
    // SAFETY: as above; a null prefetch means no flight is in progress.
    let prefetch = unsafe { prefetch.as_ref() }.and_then(|ahead| {
        if ahead.world_from_scene.is_null() {
            return None;
        }
        // SAFETY: as above, the matrix holds 16 floats.
        let matrix = unsafe { std::slice::from_raw_parts(ahead.world_from_scene, 16) };
        Some(ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(ahead.anchor_latitude, ahead.anchor_longitude),
                altitude_meters: ahead.anchor_altitude_meters,
            },
            world_from_scene: column_major(matrix),
        })
    });
    let frame = XrFrame {
        timestamp: Duration::from_secs_f64(timestamp_seconds.max(0.0)),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(placement.anchor_latitude, placement.anchor_longitude),
                altitude_meters: placement.anchor_altitude_meters,
            },
            world_from_scene: column_major(world_from_scene),
        },
        eyes: frame_eyes,
        request_overscan: f64::from(request_overscan),
        prefetch,
    };
    {
        let _runtime = handle.runtime.enter();
        if let Err(error) = handle.map.run_xr_frame(frame) {
            tracing::error!(%error, "frame failed");
            return ptr::null();
        }
    }
    handle.frames = handle.frames.wrapping_add(1);
    // Where the map looks, now and then, so a host can check its placement.
    if handle.frames == 1 || handle.frames % 300 == 0 {
        let view_state = handle.map.view_state();
        let center = view_state.external_view().anchor.position;
        tracing::info!(
            frame = handle.frames,
            zoom = view_state.zoom().value(),
            center_latitude = center.latitude,
            center_longitude = center.longitude,
            pitch = view_state.camera().get_pitch().0.to_degrees(),
            "map view"
        );
    }
    // The host copies and presents on the map's own queue, in order after these draws, so no
    // wait is needed here; polling only runs the callbacks of finished work.
    handle.map.device().poll(wgpu::Maintain::Poll);
    let Some(texture) = handle.map.head_texture() else {
        return ptr::null();
    };
    // SAFETY: the renderer runs on Metal on this platform; the callback only reads the handle,
    // which stays alive with the texture the map owns. `raw_handle` comes from the vendored
    // wgpu-hal, see vendor-wgpu.sh.
    unsafe {
        texture.as_hal::<wgpu_hal::api::Metal, _, _>(|texture| {
            texture.map_or(ptr::null(), |texture| texture.raw_handle().as_ptr().cast())
        })
    }
}

/// Wraps a compositor colour texture as a linear render target for the map; `None` for a
/// null handle, a format other than `bgra8Unorm` or its sRGB twin, or a texture that cannot
/// be viewed in another format, which the host should then draw through a copy.
///
/// # Safety
///
/// `pointer` must be null or an `id<MTLTexture>` of a single-level 2D texture that stays
/// alive for the frame.
unsafe fn import_color_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller passes a live Metal texture; `to_owned` retains it.
    let raw = unsafe { metal::TextureRef::from_ptr(pointer.cast_mut().cast()) }.to_owned();
    let format = match raw.pixel_format() {
        metal::MTLPixelFormat::BGRA8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        metal::MTLPixelFormat::BGRA8Unorm_sRGB => wgpu::TextureFormat::Bgra8UnormSrgb,
        other => {
            tracing::warn!(?other, "colour texture format is not drawable");
            return None;
        }
    };
    if format != SURFACE_FORMAT
        && !raw
            .usage()
            .contains(metal::MTLTextureUsage::PixelFormatView)
    {
        return None;
    }
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    // SAFETY: the texture is one 2D level with one layer, which is what is described here.
    let texture = unsafe {
        let hal = wgpu_hal::metal::Device::texture_from_raw(
            raw,
            format,
            metal::MTLTextureType::D2,
            1,
            1,
            wgpu_hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        );
        device.create_texture_from_hal::<wgpu_hal::api::Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some("compositor colour"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[SURFACE_FORMAT],
            },
        )
    };
    Some(texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(SURFACE_FORMAT),
        ..Default::default()
    }))
}

/// Wraps the compositor's depth texture for the renderer; `None` for a null handle.
///
/// # Safety
///
/// `pointer` must be null or an `id<MTLTexture>` of a single-level `Depth32Float` texture
/// that stays alive for the frame.
unsafe fn import_depth_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller passes a live Metal texture; `to_owned` retains it, so the wrapper
    // releases its own reference when the frame is done and the host keeps its own.
    let raw = unsafe { metal::TextureRef::from_ptr(pointer.cast_mut().cast()) }.to_owned();
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    // SAFETY: the texture is one 2D level with one layer, which is what is described here.
    // `texture_from_raw` comes from the vendored wgpu-hal, see vendor-wgpu.sh.
    let texture = unsafe {
        let hal = wgpu_hal::metal::Device::texture_from_raw(
            raw,
            wgpu::TextureFormat::Depth32Float,
            metal::MTLTextureType::D2,
            1,
            1,
            wgpu_hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        );
        device.create_texture_from_hal::<wgpu_hal::api::Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some("compositor depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        )
    };
    Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// Frees a map made by [`maplibre_visionos_create`]; null is ignored.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`] and not have been destroyed already.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_destroy(map: *mut MaplibreVisionOSMap) {
    if map.is_null() {
        return;
    }
    // SAFETY: the caller upholds the handle contract documented above.
    let handle = unsafe { Box::from_raw(map) };
    let _runtime = handle.runtime.enter();
    drop(handle.map);
}

fn column_major(values: &[f32]) -> Matrix4<f64> {
    let v = |index: usize| f64::from(values[index]);
    Matrix4::new(
        v(0),
        v(1),
        v(2),
        v(3),
        v(4),
        v(5),
        v(6),
        v(7),
        v(8),
        v(9),
        v(10),
        v(11),
        v(12),
        v(13),
        v(14),
        v(15),
    )
}

//! C map lifecycle and host-facing data structures.
mod diagnostics;
mod frame;
use diagnostics::init_logging;
pub use frame::maplibre_visionos_render_frame;

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
    tcs::system::heap,
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
    pub(crate) map: HeadlessMap,
    width: u32,
    height: u32,
    frames: u64,
    pub(crate) opaque_environment: bool,
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
        opaque_environment: false,
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

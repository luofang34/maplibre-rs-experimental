//! C ABI of the maplibre-rs renderer for the visionOS host app.
//!
//! The host owns the compositor and hands over, per eye and frame, a view matrix and the
//! frustum tangents it will present with. The map renders into its own Metal texture and
//! returns it; the host blits it into the compositor drawable.

// Crossing the C boundary needs raw pointers; every unsafe block below is that crossing.
#![allow(unsafe_code)]

use std::{
    ffi::{c_char, c_void, CStr},
    ptr,
    sync::Once,
    time::Duration,
};

use cgmath::{Deg, Matrix4};
use maplibre::{
    coords::LatLon,
    headless::{create_headless_renderer, map::HeadlessMap},
    io::tile_json::resolve_tile_json_sources,
    plugin::Plugin,
    render::{
        frame_input::{FrameInput, ViewSource},
        view_state::{ExternalAnchor, ExternalView},
        RenderPlugin,
    },
    style::Style,
};
use metal::foreign_types::ForeignType;

/// A live map rendering into an offscreen texture.
pub struct MaplibreVisionOSMap {
    runtime: tokio::runtime::Runtime,
    map: HeadlessMap,
    width: u32,
    height: u32,
}

static LOGGING: Once = Once::new();

fn init_logging() {
    LOGGING.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .init();
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
    init_logging();
    // SAFETY: the caller upholds the string contract documented above.
    let (style_json, cache_dir) = unsafe { (c_str(style_json), c_str(cache_dir)) };
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
    let (kernel, renderer) = runtime
        .block_on(create_headless_renderer(width, height, cache_dir))
        .map_err(|error| format!("renderer: {error}"))?;
    runtime.block_on(resolve_tile_json_sources(&mut style, kernel.source_client()));
    let has_terrain = style.terrain.is_some();
    let mut plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(maplibre::background::BackgroundPlugin),
        Box::new(maplibre::vector::VectorPlugin::<maplibre::vector::DefaultVectorTransferables>::default()),
        Box::new(maplibre::sdf::SdfPlugin::<maplibre::vector::DefaultVectorTransferables>::default()),
        Box::new(maplibre::raster::RasterPlugin::<maplibre::raster::DefaultRasterTransferables>::default()),
        Box::new(maplibre::hillshade::HillshadePlugin),
    ];
    if has_terrain {
        plugins.push(Box::new(
            maplibre::terrain::TerrainPlugin::<maplibre::terrain::DefaultDemTransferables>::default(),
        ));
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
    })
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

/// Renders one frame from an eye and returns its Metal texture (`id<MTLTexture>`), owned by
/// the map and valid until the next render.
///
/// The anchor is where the local frame sits: the eye's position is measured from it in
/// metres east, north and up. `view` is a column-major 4x4 matrix from that local frame to
/// camera space (x right, y up, looking along negative z). `tangents` are the frustum's left,
/// right, top and bottom tangents; `near` and `far` its planes in metres. `timestamp_seconds`
/// drives animations and must not decrease.
///
/// # Safety
///
/// `map` must come from [`maplibre_visionos_create`]; `view` must point at 16 floats and
/// `tangents` at 4.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_render(
    map: *mut MaplibreVisionOSMap,
    anchor_latitude: f64,
    anchor_longitude: f64,
    anchor_altitude_meters: f64,
    view: *const f32,
    tangents: *const f32,
    near: f32,
    far: f32,
    timestamp_seconds: f64,
) -> *const c_void {
    // SAFETY: the caller upholds the pointer contracts documented above.
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return ptr::null();
    };
    if view.is_null() || tangents.is_null() {
        return ptr::null();
    }
    // SAFETY: as above, `view` holds 16 floats and `tangents` 4.
    let (view, tangents) = unsafe {
        (
            std::slice::from_raw_parts(view, 16),
            std::slice::from_raw_parts(tangents, 4),
        )
    };
    let view = column_major(view);
    let far = if far.is_finite() && far > near { far } else { near * 1.0e7 };
    let projection = cgmath::frustum(
        f64::from(-tangents[0] * near),
        f64::from(tangents[1] * near),
        f64::from(-tangents[3] * near),
        f64::from(tangents[2] * near),
        f64::from(near),
        f64::from(far),
    );
    let external = ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(anchor_latitude, anchor_longitude),
            altitude_meters: anchor_altitude_meters,
        },
        view,
        projection,
    };
    {
        let _runtime = handle.runtime.enter();
        let input: &mut FrameInput = handle.map.frame_input_mut();
        input.timestamp = Duration::from_secs_f64(timestamp_seconds.max(0.0));
        input.view = ViewSource::External(external);
        if let Err(error) = handle.map.run_frame() {
            tracing::error!(%error, "frame failed");
            return ptr::null();
        }
    }
    // The host copies the texture on its own queue right after this returns.
    handle.map.device().poll(wgpu::Maintain::Wait);
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

//! Bounded C transport for rendered-symbol queries.
use crate::MaplibreVisionOSMap;
use std::ffi::c_char;

/// Copies the rendered-symbol query JSON into the supplied buffer.
/// Returns the required capacity including the trailing NUL, or zero on failure.
/// A null or undersized buffer is not written.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread. A non-null buffer
/// must be writable for `capacity` bytes and must not overlap the map.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_query_symbols(
    map: *const MaplibreVisionOSMap,
    x: f64,
    y: f64,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    // SAFETY: the caller provides a live handle as required above.
    let Some(handle) = (unsafe { map.as_ref() }) else {
        return 0;
    };
    let hits = handle.map.query_rendered_symbols([x, y], None);
    let bytes = match serde_json::to_vec(&hits) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "symbol query serialization failed");
            return 0;
        }
    };
    let required = bytes.len().saturating_add(1);
    if !buffer.is_null() && capacity >= required {
        // SAFETY: the capacity check and the caller's buffer contract cover this copy and terminator.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
            buffer.add(bytes.len()).write(0);
        }
    }
    required
}

/// Sets continuous sky coverage for full immersion.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_opaque_environment(
    map: *mut MaplibreVisionOSMap,
    opaque: bool,
) {
    // SAFETY: the caller provides a live, exclusively accessed handle.
    if let Some(handle) = unsafe { map.as_mut() } {
        handle.opaque_environment = opaque;
    }
}

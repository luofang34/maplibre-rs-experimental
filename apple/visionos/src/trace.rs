//! C transport for the frame timeline: the host adds what it measures to the map's frames.
use std::{ffi::c_char, time::Duration};

use maplibre::render::frame_trace::{Clock, DeviceSample};

use crate::MaplibreVisionOSMap;

/// # Safety
/// `map` must be null or a live handle accessed only by the calling render thread.
unsafe fn handle<'a>(map: *mut MaplibreVisionOSMap) -> Option<&'a mut MaplibreVisionOSMap> {
    // SAFETY: the caller upholds this function's contract.
    unsafe { map.as_mut() }
}

/// Starts a timeline of the last `capacity` frames; replaces a running one.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_trace_enable(
    map: *mut MaplibreVisionOSMap,
    capacity: u32,
) {
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    if let Some(handle) = unsafe { handle(map) } {
        handle.map.enable_frame_trace(capacity as usize);
    }
}

/// The number of the frame the map drew last, under which the host records its own spans.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_last_frame(map: *mut MaplibreVisionOSMap) -> u64 {
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    unsafe { handle(map) }.map_or(0, |handle| handle.map.last_frame_stats().frame)
}

/// Adds a span the host measured for `frame`: `gpu` for time the GPU executed, from its
/// timestamps, and false for CPU time, including encoding commands.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread; `name` must be a
/// NUL-terminated string or null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_trace_span(
    map: *mut MaplibreVisionOSMap,
    frame: u64,
    name: *const c_char,
    gpu: bool,
    nanoseconds: u64,
) {
    if name.is_null() {
        return;
    }
    // SAFETY: the caller passes a NUL-terminated string, checked non-null above.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    let clock = if gpu { Clock::Gpu } else { Clock::Cpu };
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    if let Some(trace) = unsafe { handle(map) }.and_then(|handle| handle.map.frame_trace_mut()) {
        trace.record_span(frame, &name, clock, Duration::from_nanos(nanoseconds));
    }
}

/// Records when `frame` was due at the compositor and when it got there, in nanoseconds from
/// the frame's start.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_trace_presentation(
    map: *mut MaplibreVisionOSMap,
    frame: u64,
    deadline_nanoseconds: u64,
    completed_nanoseconds: u64,
) {
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    if let Some(trace) = unsafe { handle(map) }.and_then(|handle| handle.map.frame_trace_mut()) {
        trace.record_presentation(
            frame,
            Duration::from_nanos(deadline_nanoseconds),
            Duration::from_nanos(completed_nanoseconds),
        );
    }
}

/// Records the process's resident bytes (0 when unknown) and the thermal state (negative
/// when unknown) during `frame`.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_trace_device(
    map: *mut MaplibreVisionOSMap,
    frame: u64,
    resident_bytes: u64,
    thermal_state: i32,
) {
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    if let Some(trace) = unsafe { handle(map) }.and_then(|handle| handle.map.frame_trace_mut()) {
        trace.record_device(
            frame,
            DeviceSample {
                resident_bytes: (resident_bytes > 0).then_some(resident_bytes),
                thermal_state: u8::try_from(thermal_state).ok(),
            },
        );
    }
}

/// Copies the timeline and its summary as JSON into `buffer` and empties the timeline.
/// Returns the required capacity including the trailing NUL, or zero without a timeline; a
/// null or undersized buffer is not written and the timeline is kept.
///
/// # Safety
/// The map must be live and exclusively accessed by its render thread. A non-null buffer
/// must be writable for `capacity` bytes and must not overlap the map.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_trace_export(
    map: *mut MaplibreVisionOSMap,
    buffer: *mut c_char,
    capacity: usize,
) -> usize {
    // SAFETY: the exported function's contract requires a live, exclusive handle or null.
    let Some(trace) = unsafe { handle(map) }.and_then(|handle| handle.map.frame_trace_mut()) else {
        return 0;
    };
    let bytes = match serde_json::to_vec(&trace.peek()) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "frame trace serialization failed");
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
        trace.export();
    }
    required
}

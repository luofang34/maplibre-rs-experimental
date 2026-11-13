//! Scroll input turned into zoom as GL JS's scroll zoom turns it: the device a scroll comes
//! from sets its rate, and each frame's scroll is scaled through a sigmoid.

use std::time::Duration;

use instant::Instant;

use crate::input::events::MouseScrollDelta;

/// Pixels one line of a wheel counts as: a browser's line as GL JS counts it, and macOS's
/// lines as Chrome counts them, a slow notch of a tenth of a line making one wheel step;
/// elsewhere a line is a notch, and counts as the pixels a browser reports for a notch.
#[cfg(any(target_arch = "wasm32", target_os = "macos"))]
const PIXELS_PER_LINE: f64 = 40.0;
#[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
const PIXELS_PER_LINE: f64 = 100.0;
/// GL JS's zoom rates per pixel of scroll: a trackpad's, and a mouse wheel's past one notch
/// of its smallest step.
const TRACKPAD_ZOOM_RATE: f64 = 1.0 / 100.0;
const WHEEL_ZOOM_RATE: f64 = 1.0 / 450.0;
/// The step of the pixel deltas of a mouse wheel in some browsers, which no trackpad sends.
const WHEEL_ZOOM_DELTA: f64 = 4.000244140625;
/// Pixel deltas below this come from a trackpad alone.
const TRACKPAD_MAX_DELTA: f64 = 4.0;
/// A scroll after this quiet time starts a new gesture whose device is not known yet.
const NEW_GESTURE_GAP: Duration = Duration::from_millis(400);
/// How long the first event of a gesture waits for a second one that tells its device; alone
/// it counts as a wheel's.
const FIRST_EVENT_WAIT: Duration = Duration::from_millis(40);
/// Below this product of milliseconds since the last event and pixels, a gesture is a
/// trackpad's: many small deltas in quick succession.
const TRACKPAD_PACE: f64 = 200.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ScrollDevice {
    Wheel,
    Trackpad,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Scroll {
    device: ScrollDevice,
    pixels: f64,
}

impl Scroll {
    fn zoom(self, sensitivity: f64) -> f64 {
        let rate = match self.device {
            ScrollDevice::Wheel if self.pixels.abs() > WHEEL_ZOOM_DELTA => WHEEL_ZOOM_RATE,
            _ => TRACKPAD_ZOOM_RATE,
        };
        let pixels = self.pixels * sensitivity;
        let scale = 2.0 / (1.0 + (-(pixels * rate).abs()).exp());
        scale.log2().copysign(pixels)
    }
}

/// The scroll input received since the last frame, and what the gesture tells of its device.
#[derive(Default)]
pub(super) struct ScrollInput {
    frame: Option<Scroll>,
    device: Option<ScrollDevice>,
    last_event: Option<Instant>,
    /// The first event of a gesture whose device is not known yet, with when it came.
    held: Option<(f64, Instant)>,
    sensitivity: f64,
}

impl ScrollInput {
    pub(super) fn new(sensitivity: f64) -> Self {
        Self {
            sensitivity,
            ..Self::default()
        }
    }

    pub(super) fn set_sensitivity(&mut self, sensitivity: f64) {
        if sensitivity.is_finite() && sensitivity > 0.0 {
            self.sensitivity = sensitivity;
        }
    }

    pub(super) fn sensitivity(&self) -> f64 {
        self.sensitivity
    }

    /// Adds a scroll event in logical coordinates (`LineDelta` or logical `PixelDelta`).
    pub(super) fn add(&mut self, delta: &MouseScrollDelta, now: Instant) {
        match *delta {
            MouseScrollDelta::LineDelta { y, .. } => {
                self.last_event = Some(now);
                self.device = Some(ScrollDevice::Wheel);
                self.push(ScrollDevice::Wheel, f64::from(y) * PIXELS_PER_LINE);
            }
            MouseScrollDelta::PixelDelta { y, .. } => {
                self.add_pixels(y, now);
            }
        }
    }

    fn add_pixels(&mut self, pixels: f64, now: Instant) {
        let gap = self
            .last_event
            .map(|last| now.saturating_duration_since(last));
        self.last_event = Some(now);
        if pixels != 0.0 && is_wheel_step(pixels) {
            self.device = Some(ScrollDevice::Wheel);
        } else if pixels != 0.0 && pixels.abs() < TRACKPAD_MAX_DELTA {
            self.device = Some(ScrollDevice::Trackpad);
        } else if gap.is_none_or(|gap| gap > NEW_GESTURE_GAP) {
            self.release_held();
            self.device = None;
            self.held = Some((pixels, now));
            return;
        } else if self.device.is_none() {
            let pace = gap.map_or(0.0, |gap| gap.as_secs_f64() * 1000.0) * pixels.abs();
            self.device = Some(if pace < TRACKPAD_PACE {
                ScrollDevice::Trackpad
            } else {
                ScrollDevice::Wheel
            });
        }
        let Some(device) = self.device else {
            return;
        };
        let held = self.held.take().map_or(0.0, |(pixels, _)| pixels);
        self.push(device, held + pixels);
    }

    pub(super) fn take_zoom(&mut self, now: Instant) -> Option<f64> {
        if self
            .held
            .is_some_and(|(_, at)| now.saturating_duration_since(at) >= FIRST_EVENT_WAIT)
        {
            self.release_held();
        }
        let sensitivity = self.sensitivity;
        self.frame.take().map(|scroll| scroll.zoom(sensitivity))
    }

    pub(super) fn next_wait(&self, now: Instant) -> Option<Duration> {
        self.held
            .map(|(_, at)| FIRST_EVENT_WAIT.saturating_sub(now.saturating_duration_since(at)))
    }

    fn release_held(&mut self) {
        if let Some((pixels, _)) = self.held.take() {
            self.device = Some(ScrollDevice::Wheel);
            self.push(ScrollDevice::Wheel, pixels);
        }
    }

    fn push(&mut self, device: ScrollDevice, pixels: f64) {
        let before = self.frame.map_or(0.0, |scroll| scroll.pixels);
        self.frame = Some(Scroll {
            device,
            pixels: before + pixels,
        });
    }
}

fn is_wheel_step(pixels: f64) -> bool {
    let steps = pixels / WHEEL_ZOOM_DELTA;
    (steps - steps.round()).abs() <= 1e-9 * steps.abs().max(1.0)
}

#[cfg(test)]
mod tests;

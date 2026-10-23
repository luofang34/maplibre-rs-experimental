//! Scroll input turned into zoom as GL JS's scroll zoom turns it: the device a scroll comes
//! from sets its rate, and each frame's scroll is scaled through a sigmoid.

use std::time::Duration;

use instant::Instant;
use winit::event::MouseScrollDelta;

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

/// The device scroll input came from, which sets its zoom rate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScrollDevice {
    Wheel,
    Trackpad,
}

/// Scroll input of one frame, in logical pixels before the sensitivity, positive towards
/// zooming in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scroll {
    device: ScrollDevice,
    pixels: f64,
}

impl Scroll {
    /// The zoom change, by GL JS's sigmoid of the frame's scroll: small scrolls move the map
    /// in proportion, and a frame doubles or halves its scale at most. The rate is chosen by
    /// the scroll as the device sent it, as GL JS chooses it.
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

    /// Adds a scroll event: a wheel's lines as pixels, and physical pixels as logical ones,
    /// the units GL JS's rates apply to.
    pub(super) fn add(&mut self, delta: &MouseScrollDelta, scale_factor: f64, now: Instant) {
        match delta {
            MouseScrollDelta::LineDelta(_horizontal, vertical) => {
                self.last_event = Some(now);
                self.device = Some(ScrollDevice::Wheel);
                self.push(ScrollDevice::Wheel, f64::from(*vertical) * PIXELS_PER_LINE);
            }
            MouseScrollDelta::PixelDelta(position) => {
                self.add_pixels(position.y / scale_factor, now);
            }
        }
    }

    /// Tells the device of pixel deltas as GL JS's scroll zoom does, which sees the same
    /// deltas from a browser.
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

    /// The zoom change of the frame's scroll, if any; the first event of a gesture that no
    /// second one followed in time counts as a wheel's.
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

    fn release_held(&mut self) {
        if let Some((pixels, _)) = self.held.take() {
            self.device = Some(ScrollDevice::Wheel);
            self.push(ScrollDevice::Wheel, pixels);
        }
    }

    /// The frame's input counts as the device that sent the last of it, as GL JS keeps one
    /// device for a gesture.
    fn push(&mut self, device: ScrollDevice, pixels: f64) {
        let before = self.frame.map_or(0.0, |scroll| scroll.pixels);
        self.frame = Some(Scroll {
            device,
            pixels: before + pixels,
        });
    }
}

/// Whether the delta is a whole number of a wheel's steps.
fn is_wheel_step(pixels: f64) -> bool {
    let steps = pixels / WHEEL_ZOOM_DELTA;
    (steps - steps.round()).abs() <= 1e-9 * steps.abs().max(1.0)
}

#[cfg(test)]
mod tests;

use std::time::Duration;

use instant::Instant;
use winit::{dpi::PhysicalPosition, event::MouseScrollDelta};

use super::ScrollInput;

// The expected zooms are GL JS's `log2(2 / (1 + exp(-|delta * rate|)))` for its scroll zoom,
// at 1/100 per pixel for a trackpad and 1/450 for a wheel.

fn pixels(logical: f64) -> MouseScrollDelta {
    // On a display of two physical pixels per logical one.
    MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, logical * 2.0))
}

fn ms(start: Instant, milliseconds: u64) -> Instant {
    start + Duration::from_millis(milliseconds)
}

fn assert_zoom(actual: Option<f64>, expected: f64, what: &str) {
    let actual = actual.unwrap_or(0.0);
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what}: zoom {actual}, GL JS {expected}"
    );
}

#[test]
fn a_browser_mouse_notch_waits_for_a_second_event_and_alone_zooms_as_a_wheel() {
    let mut input = ScrollInput::new(1.0);
    let start = Instant::now();
    input.add(&pixels(100.0), 2.0, start);
    assert_eq!(
        input.take_zoom(ms(start, 16)),
        None,
        "the device is not known yet"
    );
    assert_zoom(input.take_zoom(ms(start, 48)), 0.151_412_188, "one notch");
    input.add(&pixels(-100.0), 2.0, ms(start, 120));
    assert_zoom(
        input.take_zoom(ms(start, 130)),
        -0.151_412_188,
        "the next notch of the same gesture, out",
    );
}

#[test]
fn a_trackpad_gesture_is_told_by_its_pace_and_zooms_at_the_trackpad_rate() {
    let mut input = ScrollInput::new(1.0);
    let start = Instant::now();
    input.add(&pixels(10.0), 2.0, start);
    input.add(&pixels(10.0), 2.0, ms(start, 16));
    assert_zoom(
        input.take_zoom(ms(start, 16)),
        0.137_068_019,
        "the two events of 10 px together",
    );
    input.add(&pixels(2.0), 2.0, ms(start, 32));
    assert_zoom(input.take_zoom(ms(start, 32)), 0.014_354_817, "2 px");
}

#[test]
fn large_deltas_in_quick_succession_are_a_wheel() {
    let mut input = ScrollInput::new(1.0);
    let start = Instant::now();
    input.add(&pixels(100.0), 2.0, start);
    input.add(&pixels(100.0), 2.0, ms(start, 16));
    assert_zoom(input.take_zoom(ms(start, 16)), 0.285_266_180, "two notches");
}

#[test]
fn whole_steps_of_a_wheel_and_tiny_deltas_tell_their_device_at_once() {
    let mut input = ScrollInput::new(1.0);
    let start = Instant::now();
    input.add(&pixels(3.0 * 4.000_244_140_625), 2.0, start);
    assert_zoom(input.take_zoom(start), 0.019_108_857, "three wheel steps");
    input.add(&pixels(2.0), 2.0, ms(start, 1000));
    assert_zoom(
        input.take_zoom(ms(start, 1000)),
        0.014_354_817,
        "a 2 px trackpad delta",
    );
}

#[test]
fn a_line_is_a_wheel_notch() {
    let mut input = ScrollInput::new(1.0);
    input.add(&MouseScrollDelta::LineDelta(0.0, 1.0), 2.0, Instant::now());
    #[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
    let expected = 0.151_412_188;
    #[cfg(any(target_arch = "wasm32", target_os = "macos"))]
    let expected = 0.062_695_364;
    assert_zoom(input.take_zoom(Instant::now()), expected, "one line");
}

#[cfg(target_os = "macos")]
#[test]
fn a_slow_macos_notch_is_one_wheel_step() {
    let mut input = ScrollInput::new(1.0);
    // The 16.16 fixed-point tenth of a line macOS reports for a slow notch.
    input.add(
        &MouseScrollDelta::LineDelta(0.0, 0.100_006_1),
        2.0,
        Instant::now(),
    );
    assert_zoom(input.take_zoom(Instant::now()), 0.028_567_107, "one step");
}

#[test]
fn the_sensitivity_leaves_the_rate_to_the_scroll_the_device_sent() {
    let mut input = ScrollInput::new(0.5);
    // Two wheel steps: past one step, so at the wheel rate, though halved they are one step.
    input.add(&pixels(2.0 * 4.000_244_140_625), 2.0, Instant::now());
    assert_zoom(
        input.take_zoom(Instant::now()),
        0.006_398_119,
        "half of two steps",
    );
}

#[test]
fn a_frame_doubles_the_scale_at_most_and_the_sensitivity_scales_the_scroll() {
    let mut input = ScrollInput::new(1.0);
    input.add(&pixels(1.0), 2.0, Instant::now());
    input.add(&pixels(1e6), 2.0, Instant::now());
    assert_zoom(input.take_zoom(Instant::now()), 1.0, "a fling");
    assert_eq!(
        input.take_zoom(Instant::now()),
        None,
        "a frame without scroll"
    );
    let mut gentle = ScrollInput::new(0.5);
    let start = Instant::now();
    gentle.add(&pixels(10.0), 2.0, start);
    gentle.add(&pixels(10.0), 2.0, ms(start, 16));
    assert_zoom(
        gentle.take_zoom(ms(start, 16)),
        0.070_332_134,
        "half of 20 px",
    );
}

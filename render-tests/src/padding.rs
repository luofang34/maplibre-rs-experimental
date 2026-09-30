//! Camera padding requested by a fixture and the overlay GL JS draws for `showPadding`.

use std::path::Path;

use image::Rgba;
use serde_json::Value;

/// Padding in logical pixels from each edge of the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Padding {
    pub(super) top: f64,
    pub(super) bottom: f64,
    pub(super) left: f64,
    pub(super) right: f64,
}

impl Padding {
    fn from_value(value: &Value) -> Self {
        let edge = |name: &str| value.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        Self {
            top: edge("top"),
            bottom: edge("bottom"),
            left: edge("left"),
            right: edge("right"),
        }
    }
}

/// The padding the last `setPadding` or `easeTo` operation leaves; an `easeTo` that settles
/// before the next operation is the same as setting its padding.
pub(super) fn padding_of(operations: &[Value]) -> Padding {
    operations
        .iter()
        .filter_map(Value::as_array)
        .filter_map(|items| match items.first().and_then(Value::as_str)? {
            "setPadding" => items.get(1),
            "easeTo" => items.get(1)?.get("padding"),
            _ => None,
        })
        .next_back()
        .map_or_else(Padding::default, Padding::from_value)
}

/// Whether an `easeTo` only moves the padding, the one camera change the renderer applies.
pub(super) fn eases_only_padding(options: &Value) -> bool {
    options.as_object().is_some_and(|object| {
        object
            .keys()
            .all(|key| matches!(key.as_str(), "padding" | "duration" | "easing"))
    })
}

const CROSSHAIR_SIZE: f64 = 20.0;
const CROSSHAIR_WIDTH: f64 = 2.0;
const EDGE_WIDTH: f64 = 3.0;

/// A rectangle in logical pixels with the origin at the bottom left, as GL JS scissors it.
struct Rectangle {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// The edges and the crosshair as rectangles with their colours, for a viewport of `size`.
fn rectangles(padding: Padding, size: (u32, u32)) -> [(Rectangle, [u8; 3]); 6] {
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    let center_x = ((padding.left + width - padding.right) / 2.0).clamp(0.0, width);
    let center_y = ((padding.top + height - padding.bottom) / 2.0).clamp(0.0, height);
    let half_edge = EDGE_WIDTH / 2.0;
    [
        (
            Rectangle {
                x: 0.0,
                y: height - padding.top + half_edge,
                width,
                height: EDGE_WIDTH,
            },
            [255, 0, 0],
        ),
        (
            Rectangle {
                x: 0.0,
                y: padding.bottom + half_edge,
                width,
                height: EDGE_WIDTH,
            },
            [0, 255, 0],
        ),
        (
            Rectangle {
                x: padding.left - half_edge,
                y: 0.0,
                width: EDGE_WIDTH,
                height,
            },
            [0, 0, 255],
        ),
        (
            Rectangle {
                x: width - padding.right - half_edge,
                y: 0.0,
                width: EDGE_WIDTH,
                height,
            },
            [255, 0, 255],
        ),
        (
            Rectangle {
                x: center_x - CROSSHAIR_WIDTH / 2.0,
                y: height - center_y - CROSSHAIR_SIZE / 2.0,
                width: CROSSHAIR_WIDTH,
                height: CROSSHAIR_SIZE,
            },
            [0, 255, 255],
        ),
        (
            Rectangle {
                x: center_x - CROSSHAIR_SIZE / 2.0,
                y: height - center_y - CROSSHAIR_WIDTH / 2.0,
                width: CROSSHAIR_SIZE,
                height: CROSSHAIR_WIDTH,
            },
            [0, 255, 255],
        ),
    ]
}

/// Draws the padding edges and the padded center into `frame`, which is `pixel_ratio` times the
/// logical `size`.
pub(super) fn draw_overlay(
    frame: &Path,
    padding: Padding,
    size: (u32, u32),
    pixel_ratio: f64,
) -> Result<(), String> {
    let mut image = image::open(frame)
        .map_err(|error| format!("Cannot open frame for the padding overlay: {error}"))?
        .to_rgba8();
    let frame_height = image.height();
    for (rectangle, [red, green, blue]) in rectangles(padding, size) {
        // A scissor box takes whole pixels, so fractional edges truncate.
        let x0 = (rectangle.x * pixel_ratio).trunc() as i64;
        let y0 = (rectangle.y * pixel_ratio).trunc() as i64;
        let (w, h) = (
            (rectangle.width * pixel_ratio).trunc() as i64,
            (rectangle.height * pixel_ratio).trunc() as i64,
        );
        for column in x0.max(0)..(x0 + w).min(i64::from(image.width())) {
            // Rows count up from the bottom of the frame.
            for row in y0.max(0)..(y0 + h).min(i64::from(frame_height)) {
                let top_down = i64::from(frame_height) - 1 - row;
                image.put_pixel(
                    column as u32,
                    top_down as u32,
                    Rgba([red, green, blue, 255]),
                );
            }
        }
    }
    image
        .save(frame)
        .map_err(|error| format!("Cannot save the padding overlay: {error}"))
}

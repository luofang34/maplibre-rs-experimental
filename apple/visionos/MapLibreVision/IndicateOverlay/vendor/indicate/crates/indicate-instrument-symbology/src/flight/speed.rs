//! One primary speed coordinate with contextual values and attributed constraints.

mod envelope;
mod targets;

use crate::{fixed_str::FixedStr, fmt_label, palette, safety};
use indicate_instrument_scene::{Anchor, Rgba8, SceneError, SceneWriter};
use indicate_instrument_state::flight::{
    presentation,
    speed::{SpeedCoordinate, SpeedValue},
};
use indicate_instrument_state::{GroupId, PanelData, SignalStatus};

/// Geometry of one speed tape, increasing upward.
#[derive(Debug, Clone, Copy)]
pub struct SpeedLayout {
    /// Axis position and current-speed height.
    pub center: [f32; 2],
    /// Visible distance above and below current speed.
    pub half_height: f32,
    /// Horizontal center of the primary readout.
    pub readout_x: f32,
    /// Primary readout text size.
    pub text_size: f32,
    /// Caption size fitted to the available instrument width.
    pub label_size: f32,
    /// Width of the primary readout's background when supplied.
    pub readout_width: f32,
    /// Optional opaque readout background for fixed displays.
    pub readout_background: Option<Rgba8>,
    /// Pixel distance between major ticks.
    pub tick_step_pixels: f32,
    /// Primary measurement and scale color.
    pub color: Rgba8,
}

/// Formats a native speed value without changing its coordinate identity.
pub fn value_text(value: SpeedValue) -> FixedStr<24> {
    let display = value.coordinate.display_value(value.value);
    if display.abs() >= 10000.0 {
        fmt_label!(24, "{:.2E}", display)
    } else if value.coordinate == SpeedCoordinate::Mach {
        fmt_label!(24, "{:.3}", display)
    } else {
        fmt_label!(24, "{:.0}", display)
    }
}

/// Draws the primary speed, its coordinate, and one contextual speed.
pub fn readout(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
    caption_size: f32,
    width: f32,
    color: Rgba8,
) -> Result<(), SceneError> {
    let (coordinate, signal, group) = presentation::current_speed(data);
    scene.fill_color(color)?;
    scene.text(
        center[0],
        center[1] - size,
        caption_size,
        Anchor::CENTER,
        coordinate.label(),
    )?;
    if signal.status == SignalStatus::Valid && signal.value.is_finite() {
        let text = if coordinate == SpeedCoordinate::Mach {
            fmt_label!(24, "{:.3}", signal.value)
        } else if signal.value.abs() >= 10000.0 {
            fmt_label!(24, "{:.2E}", signal.value)
        } else {
            fmt_label!(24, "{:.0}", signal.value)
        };
        scene.text_attributed(
            group.to_u8(),
            center[0],
            center[1],
            fitted_size(text.as_str(), size, width),
            Anchor::CENTER,
            text.as_str(),
        )?;
    } else {
        scene.fill_color(if signal.status == SignalStatus::Failed {
            safety::FAILURE_RED
        } else {
            safety::CAUTION_AMBER
        })?;
        let label = match signal.status {
            SignalStatus::Stale => "STALE",
            SignalStatus::Degraded => "DEGRADED",
            SignalStatus::Failed => "FAIL",
            _ => "---",
        };
        scene.text(
            center[0],
            center[1],
            fitted_size(label, size * 0.65, width),
            Anchor::CENTER,
            label,
        )?;
    }
    secondary(
        scene,
        data,
        [center[0], center[1] + size],
        caption_size,
        width,
    )
}

fn secondary(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
    width: f32,
) -> Result<(), SceneError> {
    let speed = data.flight.speed;
    if !data.flight.speed_present || speed.status != SignalStatus::Valid {
        return Ok(());
    }
    let Some(value) = speed.value.secondary.filter(|v| v.is_valid()) else {
        return Ok(());
    };
    let text = fmt_label!(
        40,
        "{} {}",
        value.coordinate.label(),
        value_text(value).as_str()
    );
    scene.fill_color(palette::WHITE)?;
    scene.text_attributed(
        GroupId::SpeedPresentation.to_u8(),
        center[0],
        center[1],
        fitted_size(text.as_str(), size, width),
        Anchor::CENTER,
        text.as_str(),
    )
}

/// Draws a supplied coordinate, trend, target markers, and envelope without creating a layer.
pub fn draw(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
) -> Result<(), SceneError> {
    let (coordinate, current, _) = presentation::current_speed(data);
    if current.status == SignalStatus::Valid
        && current.value.is_finite()
        && current.value.abs() < 1_000_000.0
    {
        ladder(scene, data, layout, coordinate, current.value)?;
        trend(scene, data, layout, coordinate, current.value)?;
        envelope::draw(scene, data, layout, coordinate, current.value)?;
    }
    if let Some(color) = layout.readout_background {
        scene.fill_color(color)?;
        scene.rect(
            indicate_instrument_scene::PaintMode::Fill,
            layout.readout_x - layout.readout_width / 2.0,
            layout.center[1] - layout.text_size * 1.5,
            layout.readout_width,
            layout.text_size * 3.0,
        )?;
    }
    readout(
        scene,
        data,
        [layout.readout_x, layout.center[1]],
        layout.text_size,
        layout.label_size,
        layout.readout_width,
        layout.color,
    )?;
    target_markers(scene, data, layout)?;
    target_readouts(
        scene,
        data,
        [
            layout.readout_x,
            layout.center[1]
                - layout.half_height
                - 34.0_f32.max(
                    layout.label_size * 2.1 + layout.text_size * 0.25 + text_margin(layout) * 2.0,
                ),
        ],
        layout.label_size,
        layout.readout_width,
    )?;
    envelope_summary(
        scene,
        data,
        [
            layout.readout_x,
            layout.center[1]
                + layout.half_height
                + 15.0_f32.max(
                    layout.label_size * 0.5 + layout.text_size * 0.25 + text_margin(layout) * 2.0,
                ),
        ],
        layout.label_size,
    )
}

/// Shows boundary causes and values even when the limits lie beyond the visible tape.
pub fn envelope_summary(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
) -> Result<(), SceneError> {
    envelope::summary(scene, data, center, size)
}

/// Draws selected and effective target readouts for a compact instrument.
pub fn target_readouts(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
    width: f32,
) -> Result<(), SceneError> {
    targets::readouts(scene, data, center, size, width)
}

/// Draws selected and identity-matched effective markers on a supplied speed scale.
pub fn target_markers(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
) -> Result<(), SceneError> {
    targets::markers(scene, data, layout)
}

fn ladder(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
    coordinate: SpeedCoordinate,
    current: f32,
) -> Result<(), SceneError> {
    let [x, center] = layout.center;
    let step = interval(coordinate);
    let base = (current / step) as i32 as f32;
    let group = if data.flight.speed_present {
        GroupId::SpeedPresentation
    } else {
        GroupId::Air
    };
    scene.stroke(layout.color, 1.3)?;
    scene.fill_color(layout.color)?;
    scene.line(
        x,
        center - layout.half_height,
        x,
        center + layout.half_height,
    )?;
    let window = (layout.text_size * 1.6)
        .max(layout.text_size * 1.25 + layout.label_size * 0.5 + text_margin(layout) * 2.0);
    for index in -6..=6 {
        let value = (base + index as f32) * step;
        let offset = (value - current) / step * layout.tick_step_pixels;
        if value < 0.0 || offset.abs() > layout.half_height || offset.abs() < window {
            continue;
        }
        let y = center - offset;
        scene.line(x - 9.0, y, x, y)?;
        let text = if coordinate == SpeedCoordinate::Mach {
            fmt_label!(16, "{:.2}", value)
        } else {
            fmt_label!(16, "{:.0}", value)
        };
        scene.text_attributed(
            group.to_u8(),
            x - 14.0,
            y,
            layout.text_size * 0.5,
            Anchor::MIDDLE_RIGHT,
            text.as_str(),
        )?;
    }
    scene.polyline(&[
        [x - 9.0, center - 6.0],
        [x, center],
        [x - 9.0, center + 6.0],
    ])
}

fn trend(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
    coordinate: SpeedCoordinate,
    current: f32,
) -> Result<(), SceneError> {
    if data.presentation.unusual || data.flight.speed.status != SignalStatus::Valid {
        return Ok(());
    }
    let Some(rate) = data.flight.speed.value.rate else {
        return Ok(());
    };
    let delta = coordinate.display_value(rate) * 6.0;
    if !delta.is_finite() {
        return Ok(());
    }
    let y = position(layout, coordinate, current, current + delta);
    scene.stroke(palette::MAGENTA, 2.0)?;
    scene.line(
        layout.center[0] + 4.0,
        layout.center[1],
        layout.center[0] + 4.0,
        y,
    )
}

fn interval(coordinate: SpeedCoordinate) -> f32 {
    match coordinate {
        SpeedCoordinate::Mach => 0.02,
        SpeedCoordinate::DynamicPressure => 1000.0,
        _ => 10.0,
    }
}

fn position(
    layout: SpeedLayout,
    coordinate: SpeedCoordinate,
    current: f32,
    display_value: f32,
) -> f32 {
    layout.center[1]
        - ((display_value - current) / interval(coordinate) * layout.tick_step_pixels)
            .clamp(-layout.half_height, layout.half_height)
}

fn fitted_size(text: &str, size: f32, width: f32) -> f32 {
    let advance = indicate_instrument_scene::nominal_text_width(size, text.chars().count());
    if advance > width {
        size * width / advance
    } else {
        size
    }
}

// Transparent readouts need space for their contrast halos; an opaque box masks underlying ticks.
fn text_margin(layout: SpeedLayout) -> f32 {
    if layout.readout_background.is_none() {
        2.0
    } else {
        0.0
    }
}

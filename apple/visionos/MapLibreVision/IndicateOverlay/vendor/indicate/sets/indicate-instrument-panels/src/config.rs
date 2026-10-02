//! The airframe-configuration panel: flap position and trim.
//!
//! Configuration is what the airframe is set to, which is a different
//! question from what it is doing, so it gets its own surface rather
//! than a corner of the PFD. It is a conventional-instrument function:
//! a comparison and reversion surface, with no primary-flight credit.
//!
//! Each scale draws only when its own value shows. A vehicle with a flap
//! sensor and no trim sensor gets one scale and a dash where the other
//! would be, because an absent sensor is not a centred pointer.

use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::DesignFrame;
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, SceneError, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData, SignalStatus};
use indicate_instrument_symbology::{annunciation, fmt_label, palette, safety, status_paint};

/// Left edge of the scales.
const SCALE_X: f32 = 150.0;
/// Length of a scale in the direction it travels.
const SCALE_LEN: f32 = 220.0;
/// Half-thickness of a scale's pointer.
const POINTER_HALF: f32 = 7.0;

/// Draws the configuration panel from resolved state.
///
/// Layers: `Background` carries the opaque ground (the panel declares
/// `Opaque`), `Tapes` the scales and their labels, `Annunciation` the
/// status flags — the same failure semantics as every other panel.
pub fn draw_config(
    data: &PanelData,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), SceneError> {
    let config = &data.airframe;

    scene.begin_layer(LayerId::Background)?;
    scene.fill_color(palette::BLACK)?;
    scene.rect(PaintMode::Fill, 0.0, 0.0, frame.width, frame.height)?;
    scene.end_layer(LayerId::Background)?;

    scene.begin_layer(LayerId::Tapes)?;
    let shown = config.status.shows_value();
    flap_scale(scene, shown.then_some(config.value.flap_ratio).flatten(), {
        // A selected detent draws only beside a sensed position: a
        // selection with nothing to compare it against is a number, not
        // a configuration.
        shown.then_some(config.value.flap_selected_ratio).flatten()
    })?;
    trim_scale(
        scene,
        shown.then_some(config.value.elevator_trim_ratio).flatten(),
    )?;
    scene.end_layer(LayerId::Tapes)?;

    scene.begin_layer(LayerId::Annunciation)?;
    match config.status {
        SignalStatus::Failed => {
            status_paint::draw_red_x(scene, 0.0, 0.0, frame.width, frame.height, "CFG")?;
        }
        SignalStatus::Stale | SignalStatus::Degraded => {
            status_paint::draw_flag(scene, frame.width - 60.0, 36.0, "CFG")?;
        }
        SignalStatus::Missing | SignalStatus::Valid => {}
    }
    if let Some(alerts) = alerts {
        annunciation::draw_alert_stack(scene, alerts)?;
    }
    scene.end_layer(LayerId::Annunciation)?;
    Ok(())
}

/// The flap scale: retracted at the top, fully extended at the bottom,
/// which is the direction the surface itself travels.
///
/// The selected detent draws as a separate cyan mark rather than moving
/// the sensed pointer, so a flap in transit reads as two marks apart and
/// a flap that never reached its detent reads as two marks that stay
/// apart.
fn flap_scale(
    scene: &mut SceneWriter<'_>,
    sensed: Option<f32>,
    selected: Option<f32>,
) -> Result<(), SceneError> {
    let top = 70.0;
    scene.fill_color(palette::GREY)?;
    scene.text(
        SCALE_X - 40.0,
        top - 24.0,
        16.0,
        Anchor::MIDDLE_LEFT,
        "FLAP",
    )?;
    scene.stroke(palette::GREY, 2.0)?;
    scene.line(SCALE_X, top, SCALE_X, top + SCALE_LEN)?;
    for step in 0..=4u8 {
        let y = top + SCALE_LEN * f32::from(step) / 4.0;
        scene.line(SCALE_X - 8.0, y, SCALE_X, y)?;
    }

    let Some(ratio) = sensed else {
        scene.fill_color(safety::FAILURE_RED)?;
        scene.text(
            SCALE_X + 16.0,
            top + SCALE_LEN / 2.0,
            18.0,
            Anchor::MIDDLE_LEFT,
            "---",
        )?;
        return Ok(());
    };
    if let Some(detent) = selected {
        let y = top + SCALE_LEN * detent;
        scene.fill_color(palette::CYAN)?;
        scene.polygon(
            PaintMode::Fill,
            &[
                [SCALE_X - 10.0, y],
                [SCALE_X - 22.0, y - POINTER_HALF],
                [SCALE_X - 22.0, y + POINTER_HALF],
            ],
        )?;
    }
    let y = top + SCALE_LEN * ratio;
    scene.fill_color(palette::WHITE)?;
    scene.polygon(
        PaintMode::Fill,
        &[
            [SCALE_X + 10.0, y],
            [SCALE_X + 22.0, y - POINTER_HALF],
            [SCALE_X + 22.0, y + POINTER_HALF],
        ],
    )?;
    let text = fmt_label!(8, "{:.0}", ratio * 100.0);
    scene.text_attributed(
        GroupId::AirframeConfig.to_u8(),
        SCALE_X + 34.0,
        y,
        18.0,
        Anchor::MIDDLE_LEFT,
        text.as_str(),
    )
}

/// The elevator-trim scale: nose-down left, nose-up right, with the
/// neutral mark drawn longer so a centred trim is readable without the
/// numerals.
fn trim_scale(scene: &mut SceneWriter<'_>, ratio: Option<f32>) -> Result<(), SceneError> {
    let y = 320.0;
    let left = SCALE_X - SCALE_LEN / 2.0;
    scene.fill_color(palette::GREY)?;
    scene.text(left, y - 30.0, 16.0, Anchor::MIDDLE_LEFT, "TRIM")?;
    scene.stroke(palette::GREY, 2.0)?;
    scene.line(left, y, left + SCALE_LEN, y)?;
    scene.line(
        left + SCALE_LEN / 2.0,
        y - 12.0,
        left + SCALE_LEN / 2.0,
        y + 12.0,
    )?;

    let Some(ratio) = ratio else {
        scene.fill_color(safety::FAILURE_RED)?;
        scene.text(left + SCALE_LEN + 16.0, y, 18.0, Anchor::MIDDLE_LEFT, "---")?;
        return Ok(());
    };
    let x = left + SCALE_LEN / 2.0 + SCALE_LEN / 2.0 * ratio;
    scene.fill_color(palette::WHITE)?;
    scene.polygon(
        PaintMode::Fill,
        &[
            [x, y - 10.0],
            [x - POINTER_HALF, y - 22.0],
            [x + POINTER_HALF, y - 22.0],
        ],
    )?;
    let text = fmt_label!(8, "{:.0}", ratio * 100.0);
    scene.text_attributed(
        GroupId::AirframeConfig.to_u8(),
        left + SCALE_LEN + 16.0,
        y,
        18.0,
        Anchor::MIDDLE_LEFT,
        text.as_str(),
    )
}

#[cfg(test)]
mod tests;

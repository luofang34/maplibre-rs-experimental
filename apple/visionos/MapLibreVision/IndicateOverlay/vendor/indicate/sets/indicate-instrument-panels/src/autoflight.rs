//! The autoflight annunciator: what the automation is doing, and what
//! it is about to do.
//!
//! Active and armed modes sit in one column per axis, active above
//! armed, so the pair reads as a sequence rather than as two unrelated
//! labels. Active modes wear their own color and armed modes wear
//! annunciation white, which is not themable: the distinction between
//! what is flying the aircraft now and what will fly it next survives
//! any theme a shell supplies.
//!
//! The modes and the targets answer to separate groups, so they blank
//! separately. An automation that reports its modes and no targets
//! shows modes and dashes, never modes beside numbers nobody sent.

use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::DesignFrame;
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, SceneError, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData, Sig};
use indicate_instrument_symbology::{annunciation, fmt_label, palette, safety};

/// Left margin every row shares.
const TEXT_X: f32 = 24.0;
/// Where the lateral column starts.
const LATERAL_X: f32 = 150.0;
/// Where the vertical column starts.
const VERTICAL_X: f32 = 300.0;
/// Baseline of the first target row.
const FIRST_TARGET_Y: f32 = 190.0;
/// Distance between target rows.
const TARGET_H: f32 = 46.0;

/// Draws the autoflight annunciator from resolved state.
///
/// Layers: `Background` carries the opaque ground (the panel declares
/// `Opaque`), `Tapes` the mode annunciations and target readouts,
/// `Annunciation` the status flags — the failure semantics every panel
/// in this set shares.
pub fn draw_autoflight(
    data: &PanelData,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), SceneError> {
    scene.begin_layer(LayerId::Background)?;
    scene.fill_color(palette::BLACK)?;
    scene.rect(PaintMode::Fill, 0.0, 0.0, frame.width, frame.height)?;
    scene.end_layer(LayerId::Background)?;

    scene.begin_layer(LayerId::Tapes)?;
    column_headings(scene)?;
    target_rows(scene, data)?;
    scene.end_layer(LayerId::Tapes)?;

    scene.begin_layer(LayerId::Annunciation)?;
    use indicate_instrument_symbology::flight::modes;
    modes::draw(
        scene,
        data,
        modes::FmaLayout {
            center: [240.0, 70.0],
            column_step: 125.0,
            row_step: 28.0,
            text_size: 20.0,
            attention_ms: 5000.0,
        },
    )?;

    if let Some(alerts) = alerts {
        annunciation::draw_alert_stack(scene, alerts)?;
    }
    scene.end_layer(LayerId::Annunciation)?;
    Ok(())
}

/// The fixed furniture: what each column is for. Drawn whatever the
/// groups say, so an empty panel still reads as an autoflight panel
/// rather than as a panel that failed to load.
fn column_headings(scene: &mut SceneWriter<'_>) -> Result<(), SceneError> {
    scene.fill_color(palette::GREY)?;
    scene.text(240.0, 14.0, 16.0, Anchor::CENTER, "AFCS")?;
    scene.text(115.0, 36.0, 14.0, Anchor::CENTER, "LATERAL")?;
    scene.text(240.0, 36.0, 14.0, Anchor::CENTER, "VERTICAL")?;
    scene.text(365.0, 36.0, 14.0, Anchor::CENTER, "THRUST")?;
    scene.text(TEXT_X, 154.0, 16.0, Anchor::MIDDLE_LEFT, "TARGETS")
}

/// The three target readouts, each gated by its own signal.
fn target_rows(scene: &mut SceneWriter<'_>, data: &PanelData) -> Result<(), SceneError> {
    let targets = &data.flight.ap_targets;
    // The row names carry the SEL prefix so no row name is also a mode
    // label: "ALT" beside "ALT" would leave the reader deciding which
    // one is the mode the automation is in.
    if data.flight.speed_present || data.flight.guidance_present {
        indicate_instrument_symbology::flight::speed::target_readouts(
            scene,
            data,
            [240.0, 175.0],
            16.0,
            400.0,
        )?;
    } else {
        target_row(scene, 0, "SEL IAS", targets.airspeed_kt, "kt")?;
    }
    target_row(scene, 1, "SEL ALT", targets.altitude_ft, "FT")?;
    target_row(scene, 2, "SEL VS", targets.vertical_speed_fpm, "FPM")
}

/// One target row: its name, its value, and its unit.
///
/// The unit is drawn beside the value rather than folded into it, so a
/// row whose value is dashed still says what the number would have
/// been measured in.
fn target_row(
    scene: &mut SceneWriter<'_>,
    row: usize,
    name: &str,
    value: Sig<f32>,
    unit: &str,
) -> Result<(), SceneError> {
    let y = FIRST_TARGET_Y + row as f32 * TARGET_H;
    scene.fill_color(palette::GREY)?;
    scene.text(TEXT_X, y, 18.0, Anchor::MIDDLE_LEFT, name)?;
    if value.status.shows_value() {
        let text = fmt_label!(8, "{:.0}", value.value);
        scene.fill_color(palette::CYAN)?;
        scene.text_attributed(
            GroupId::ApTargets.to_u8(),
            LATERAL_X,
            y,
            22.0,
            Anchor::MIDDLE_LEFT,
            text.as_str(),
        )?;
    } else {
        scene.fill_color(safety::ANNUNCIATION_WHITE)?;
        scene.text(LATERAL_X, y, 22.0, Anchor::MIDDLE_LEFT, "---")?;
    }
    scene.fill_color(palette::GREY)?;
    scene.text(VERTICAL_X, y, 16.0, Anchor::MIDDLE_LEFT, unit)
}

#[cfg(test)]
mod tests;

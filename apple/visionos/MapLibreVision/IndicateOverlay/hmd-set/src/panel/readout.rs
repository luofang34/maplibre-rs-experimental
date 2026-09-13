//! Shared value and failure presentation for forward and off-axis instruments.
use super::*;

pub(super) fn value(
    scene: &mut SceneWriter<'_>,
    label: &str,
    signal: Sig<f32>,
    group: GroupId,
    position: [f32; 2],
    size: f32,
) -> Result<(), PanelDrawError> {
    let [x, y] = position;
    scene.fill_color(HUD_GREEN)?;
    scene.text(x, y - 32.0, 12.0, Anchor::CENTER, label)?;
    let finite = signal.value.is_finite() && signal.value.abs() < 1_000_000.0;
    let status = if !finite && signal.status.shows_value() {
        SignalStatus::Failed
    } else {
        signal.status
    };
    scene.fill_color(color(status))?;
    if status.shows_value() {
        let number = fmt_label!(16, "{:.0}", signal.value);
        scene.text_attributed(group.to_u8(), x, y, size, Anchor::CENTER, number.as_str())?;
    } else {
        scene.text(x, y, size, Anchor::CENTER, "---")?;
    }
    if status != SignalStatus::Valid {
        scene.text(x, y + 28.0, 12.0, Anchor::CENTER, status_label(status))?;
    }
    Ok(())
}

pub(super) fn color(status: SignalStatus) -> Rgba8 {
    match status {
        SignalStatus::Valid => HUD_GREEN,
        SignalStatus::Failed => safety::FAILURE_RED,
        _ => palette::AMBER,
    }
}

pub(super) fn status_label(status: SignalStatus) -> &'static str {
    match status {
        SignalStatus::Valid => "",
        SignalStatus::Stale => "STALE",
        SignalStatus::Degraded => "CHECK",
        SignalStatus::Failed => "FAILED",
        SignalStatus::Missing => "MISSING",
    }
}

//! Model-supplied intervals preserve gaps and boundary provenance.

use super::*;
use indicate_instrument_state::flight::envelope::{
    EnvelopeCoverage, EnvelopeInterval, EnvelopeKind,
};

pub(super) fn summary(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
) -> Result<(), SceneError> {
    if !data.flight.speed_present {
        return Ok(());
    }
    let speed = data.flight.speed;
    let envelope = speed.value.envelope;
    let status = data.flight.envelope_status;
    let note = coverage_note(status, &envelope);
    let mut row = 0usize;
    if let Some(note) = note {
        scene.fill_color(safety::CAUTION_AMBER)?;
        scene.text(center[0], center[1], size, Anchor::CENTER, note)?;
        row = row.wrapping_add(1);
    }
    if status != SignalStatus::Valid || envelope.coverage == EnvelopeCoverage::Unavailable {
        return Ok(());
    }
    let Some(current) = speed.value.current else {
        return Ok(());
    };
    let containing =
        envelope.intervals.iter().flatten().find(|v| {
            v.kind == EnvelopeKind::Operating && v.lower <= current && current <= v.upper
        });
    let Some(bounds) = containing else {
        if note.is_none() {
            scene.fill_color(safety::CAUTION_AMBER)?;
            scene.text(center[0], center[1], size, Anchor::CENTER, "OUTSIDE LIM")?;
        }
        return Ok(());
    };
    scene.fill_color(safety::ANNUNCIATION_WHITE)?;
    for (value, reason) in [
        (bounds.lower, bounds.lower_reason),
        (bounds.upper, bounds.upper_reason),
    ] {
        let value = SpeedValue {
            coordinate: speed.value.coordinate,
            value,
        };
        let label = fmt_label!(40, "{} {}", reason.label(), value_text(value).as_str());
        scene.text_attributed(
            GroupId::SpeedPresentation.to_u8(),
            center[0],
            center[1] + row as f32 * size * 1.6,
            size,
            Anchor::CENTER,
            label.as_str(),
        )?;
        row = row.wrapping_add(1);
    }
    Ok(())
}

fn coverage_note(
    status: SignalStatus,
    envelope: &indicate_instrument_state::flight::envelope::ProjectedEnvelope,
) -> Option<&'static str> {
    match status {
        SignalStatus::Failed => return Some("ENV FAIL"),
        SignalStatus::Stale => return Some("ENV STALE"),
        SignalStatus::Degraded => return Some("ENV DEGRADED"),
        SignalStatus::Missing => return Some("ENV ---"),
        SignalStatus::Valid => {}
    }
    match envelope.coverage {
        EnvelopeCoverage::Partial => Some("ENV PARTIAL"),
        EnvelopeCoverage::Unavailable | EnvelopeCoverage::Unknown => Some("ENV ---"),
        EnvelopeCoverage::Complete => (!envelope
            .intervals
            .iter()
            .flatten()
            .any(|v| v.kind == EnvelopeKind::Operating))
        .then_some("ENV EMPTY"),
    }
}

pub(super) fn draw(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
    coordinate: SpeedCoordinate,
    current: f32,
) -> Result<(), SceneError> {
    if !data.flight.speed_present {
        return Ok(());
    }
    let envelope = data.flight.speed.value.envelope;
    let unavailable = data.flight.envelope_status != SignalStatus::Valid;
    if unavailable
        || matches!(
            envelope.coverage,
            EnvelopeCoverage::Unavailable | EnvelopeCoverage::Unknown
        )
    {
        return Ok(());
    }
    for bounds in envelope.intervals.iter().flatten() {
        if data.presentation.unusual && bounds.kind != EnvelopeKind::Operating {
            continue;
        }
        interval_marks(scene, *bounds, layout, coordinate, current)?;
    }
    Ok(())
}

fn interval_marks(
    scene: &mut SceneWriter<'_>,
    bounds: EnvelopeInterval,
    layout: SpeedLayout,
    coordinate: SpeedCoordinate,
    current: f32,
) -> Result<(), SceneError> {
    let lo = coordinate.display_value(bounds.lower);
    let hi = coordinate.display_value(bounds.upper);
    if !lo.is_finite() || !hi.is_finite() {
        return Ok(());
    }
    let visible_half = layout.half_height / layout.tick_step_pixels * interval(coordinate);
    if hi < current - visible_half || lo > current + visible_half {
        return Ok(());
    }
    let (offset, color, label) = match bounds.kind {
        EnvelopeKind::Operating => (34.0, safety::ANNUNCIATION_WHITE, "LIM"),
        EnvelopeKind::Protection => (40.0, safety::CAUTION_AMBER, "PROT"),
        EnvelopeKind::Awareness => (46.0, safety::CAUTION_AMBER, "AWARE"),
        EnvelopeKind::Performance => (52.0, palette::CYAN, "PERF"),
        EnvelopeKind::Preferred => (58.0, palette::GREY, "PREF"),
        EnvelopeKind::Unknown => return Ok(()),
    };
    let x = layout.center[0] + offset;
    scene.stroke(color, 1.5)?;
    scene.line(
        x,
        position(layout, coordinate, current, lo),
        x,
        position(layout, coordinate, current, hi),
    )?;
    scene.fill_color(color)?;
    for (value, reason) in [(lo, bounds.lower_reason), (hi, bounds.upper_reason)] {
        if (value - current).abs() > visible_half {
            continue;
        }
        let y = position(layout, coordinate, current, value);
        scene.line(x - 3.0, y, x + 3.0, y)?;
        // Labels use the main limit lane; other lanes retain explicit kind labels.
        let text = if reason == indicate_instrument_state::flight::envelope::BoundaryReason::Mission
        {
            fmt_label!(32, "{}", label)
        } else {
            fmt_label!(32, "{} {}", label, reason.label())
        };
        scene.text_attributed(
            GroupId::SpeedPresentation.to_u8(),
            x + 7.0,
            y,
            layout.text_size * 0.33,
            Anchor::MIDDLE_LEFT,
            text.as_str(),
        )?;
    }
    Ok(())
}

//! Numeric aircraft references remain distinct from the world-bearing compass.
use super::*;
use indicate_instrument_state::HeadingReference;

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    let suffix = match data.heading.reference {
        HeadingReference::True => "T",
        HeadingReference::SimLocalTrue => "SIM T",
        HeadingReference::Magnetic => "M",
        HeadingReference::Unknown => "REF",
    };
    let label = fmt_label!(16, "HDG {}", suffix);
    readout::value(
        scene,
        label.as_str(),
        degrees(data.heading.value_rad),
        GroupId::Heading,
        [548.0, 85.0],
        21.0,
    )?;
    readout::value(
        scene,
        "TRK T",
        degrees(data.track_rad),
        GroupId::Kinematics,
        [702.0, 85.0],
        21.0,
    )?;
    Ok(())
}

fn degrees(mut signal: Sig<f32>) -> Sig<f32> {
    // Round before wrapping so north cannot display as 360 beside a zero-degree compass tick.
    signal.value = (libm::roundf(signal.value.to_degrees()) % 360.0 + 360.0) % 360.0;
    signal
}

#[cfg(test)]
mod tests;

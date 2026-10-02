//! Receiver deviations are fly-to indications, distinct from flight-director commands.
use super::*;
use indicate_instrument_state::{HeadingReference, NavFromTo, NavScale, NavSource};

mod deviation;

pub(super) fn draw(
    data: &PanelData,
    compact: bool,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    if data.presentation.unusual || data.nav.data.source == NavSource::None {
        return Ok(());
    }
    let center = if compact {
        [200.0, 405.0]
    } else {
        [219.0, 549.0]
    };
    if available(data) {
        let nav = &data.nav.data;
        let color = source_color(nav.source);
        deviation::draw(scene, center, nav.cdi_dots, false, color)?;
        scene.fill_color(color)?;
        scene.text(center[0], center[1] - 25.0, 12.0, Anchor::CENTER, "CDI")?;
        let label = fmt_label!(
            24,
            "{} {} {}",
            receiver(nav.source),
            nav.scale.label(),
            if nav.fromto == NavFromTo::To {
                "TO"
            } else {
                "FROM"
            }
        );
        scene.text_attributed(
            GroupId::Nav.to_u8(),
            center[0],
            center[1] + 28.0,
            12.0,
            Anchor::CENTER,
            label.as_str(),
        )?;
        vertical(data, compact, scene)?;
    } else {
        scene.fill_color(readout::color(data.nav.status))?;
        if data.nav.status == SignalStatus::Valid {
            scene.fill_color(safety::CAUTION_AMBER)?;
        }
        let label = fmt_label!(
            24,
            "NAV {}",
            if data.nav.status == SignalStatus::Valid {
                "UNAVAILABLE"
            } else {
                readout::status_label(data.nav.status)
            }
        );
        scene.text(center[0], center[1], 12.0, Anchor::CENTER, label.as_str())?;
    }
    Ok(())
}

fn available(data: &PanelData) -> bool {
    let nav = &data.nav.data;
    data.nav.status == SignalStatus::Valid
        && matches!(
            nav.source,
            NavSource::Gps | NavSource::Nav1 | NavSource::Nav2
        )
        && matches!(nav.fromto, NavFromTo::To | NavFromTo::From)
        && nav.scale != NavScale::Unknown
        && nav.course_reference != HeadingReference::Unknown
        && live(data.nav.course_rose_rad)
        && nav.cdi_dots.is_finite()
}

fn vertical(
    data: &PanelData,
    compact: bool,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    let nav = &data.nav.data;
    let Some(dots) = nav.vdev_dots else {
        return Ok(());
    };
    if !dots.is_finite() {
        return Ok(());
    }
    let center = if compact {
        [1000.0, 405.0]
    } else {
        [842.0, 286.0]
    };
    let label = if matches!(nav.source, NavSource::Nav1 | NavSource::Nav2)
        && nav.scale == NavScale::Approach
    {
        "GS"
    } else {
        "VDEV"
    };
    // A GPS path has no declared service level here, so it cannot claim an ILS glideslope or LPV.
    let color = source_color(nav.source);
    scene.fill_color(color)?;
    scene.text(center[0], center[1] - 118.0, 12.0, Anchor::CENTER, label)?;
    deviation::draw(scene, center, dots, true, color)
}

fn source_color(source: NavSource) -> Rgba8 {
    if source == NavSource::Gps {
        palette::MAGENTA
    } else {
        HUD_GREEN
    }
}

fn receiver(source: NavSource) -> &'static str {
    match source {
        NavSource::Gps => "GPS",
        NavSource::Nav1 => "NAV1",
        NavSource::Nav2 => "NAV2",
        _ => "",
    }
}

#[cfg(test)]
mod tests;

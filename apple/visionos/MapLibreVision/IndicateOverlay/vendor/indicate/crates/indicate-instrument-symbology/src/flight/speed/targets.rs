//! Selected targets remain distinct from effective guidance targets.

use super::*;

pub(super) fn markers(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: SpeedLayout,
) -> Result<(), SceneError> {
    let (coordinate, current, _) = presentation::current_speed(data);
    if current.status == SignalStatus::Valid && current.value.is_finite() {
        let selected = presentation::selected_speed(data);
        let effective = presentation::effective_speed_projection(data);
        for (target, active) in [(selected, false), (effective, true)] {
            let Some(projected) = target.and_then(|v| v.projected) else {
                continue;
            };
            let display = coordinate.display_value(projected);
            if !display.is_finite() {
                continue;
            }
            let y = position(layout, coordinate, current.value, display);
            let x = layout.center[0] + if active { 12.0 } else { 22.0 };
            scene.stroke(
                if active {
                    palette::MAGENTA
                } else {
                    palette::CYAN
                },
                if active { 2.6 } else { 1.3 },
            )?;
            scene.polyline(&[[x + 7.0, y - 5.0], [x, y], [x + 7.0, y + 5.0]])?;
        }
    }
    Ok(())
}

pub(super) fn readouts(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    center: [f32; 2],
    size: f32,
    width: f32,
) -> Result<(), SceneError> {
    for (row, target) in [
        presentation::effective_speed(data),
        presentation::selected_speed(data).map(|t| t.native),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(target) = target else { continue };
        scene.fill_color(if row == 0 {
            palette::MAGENTA
        } else {
            palette::CYAN
        })?;
        let label = fmt_label!(
            48,
            "{} {} {}",
            if row == 0 { "TGT" } else { "SEL" },
            reference_label(target.coordinate),
            value_text(target).as_str()
        );
        let group = if row == 0 {
            GroupId::Guidance
        } else if data.flight.speed_present {
            GroupId::SpeedPresentation
        } else {
            GroupId::ApTargets
        };
        scene.text_attributed(
            group.to_u8(),
            center[0],
            center[1] + row as f32 * size * 1.6,
            fitted_size(label.as_str(), size, width),
            Anchor::CENTER,
            label.as_str(),
        )?;
    }
    Ok(())
}

fn reference_label(coordinate: SpeedCoordinate) -> &'static str {
    match coordinate {
        SpeedCoordinate::Ias => "IAS",
        SpeedCoordinate::Cas => "CAS",
        SpeedCoordinate::Eas => "EAS",
        SpeedCoordinate::Mach => "M",
        SpeedCoordinate::DynamicPressure => "Q PA",
        SpeedCoordinate::Unknown => "?",
    }
}

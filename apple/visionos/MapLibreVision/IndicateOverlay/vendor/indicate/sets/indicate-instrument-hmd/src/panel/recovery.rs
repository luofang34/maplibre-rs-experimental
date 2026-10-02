//! Extreme-pitch orientation cues share the airframe's hysteresis policy.
use super::*;
use indicate_instrument_state::ChevronSense;

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    let Some(chevrons) = geometry(data) else {
        return Ok(());
    };
    scene.stroke(safety::FAILURE_RED, 3.5)?;
    for points in chevrons {
        scene.polyline(&points)?;
    }
    Ok(())
}

fn geometry(data: &PanelData) -> Option<[[[f32; 2]; 3]; 2]> {
    if !live(data.roll_rad) || !live(data.pitch_rad) || !data.presentation.unusual {
        return None;
    }
    let toward = match data.presentation.chevrons? {
        ChevronSense::HorizonBelow => 1.0,
        ChevronSense::HorizonAbove => -1.0,
    };
    let bank = data.roll_rad.value;
    let point = |x: f32, y: f32| {
        [
            600.0 + x * libm::cosf(bank) + y * libm::sinf(bank),
            405.0 - x * libm::sinf(bank) + y * libm::cosf(bank),
        ]
    };
    Some([26.0, 47.0].map(|offset| {
        let y = -toward * offset;
        [
            point(-23.0, y),
            point(0.0, y + toward * 15.0),
            point(23.0, y),
        ]
    }))
}

#[cfg(test)]
mod tests;

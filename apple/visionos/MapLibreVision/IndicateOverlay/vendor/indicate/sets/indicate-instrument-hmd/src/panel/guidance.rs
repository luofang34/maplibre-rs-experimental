//! Command bars use a bounded aircraft reference, independent of head pose.

use super::*;
use indicate_instrument_symbology::flight::director::{self, DirectorLayout};

pub(super) fn draw(
    data: &PanelData,
    compact: bool,
    navigation_visible: bool,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(LayerId::Guidance)?;
    if navigation_visible {
        navigation::draw(data, compact, scene)?;
    }
    if !compact && indicate_instrument_state::flight::presentation::attitude_error(data).is_some() {
        scene.stroke(HUD_GREEN, 1.3)?;
        scene.rect(PaintMode::Stroke, 548.0, 353.0, 104.0, 104.0)?;
        scene.line(582.0, 405.0, 594.0, 405.0)?;
        scene.line(606.0, 405.0, 618.0, 405.0)?;
        scene.circle(PaintMode::Stroke, 600.0, 405.0, 3.0)?;
        scene.fill_color(HUD_GREEN)?;
        scene.text(600.0, 473.0, 11.0, Anchor::CENTER, "FD ATTITUDE")?;
    }
    director::draw(
        scene,
        data,
        DirectorLayout {
            center: [600.0, 405.0],
            pixels_per_degree: [2.0, 1.5],
            travel: 42.0,
            length: 44.0,
            thickness: 2.5,
        },
    )?;
    scene.end_layer(LayerId::Guidance)?;
    Ok(())
}

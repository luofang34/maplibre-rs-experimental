//! Aircraft attitude command bars share one validity gate across displays.

use crate::palette;
use indicate_instrument_scene::{PaintMode, SceneError, SceneWriter};
use indicate_instrument_state::{
    PanelData, flight::presentation::attitude_error, units::RAD_TO_DEG,
};

/// Geometry of dual fly-to attitude command bars.
#[derive(Debug, Clone, Copy)]
pub struct DirectorLayout {
    /// Fixed aircraft reference position.
    pub center: [f32; 2],
    /// Pitch and bank displacement in pixels per degree.
    pub pixels_per_degree: [f32; 2],
    /// Maximum displacement from the aircraft reference.
    pub travel: f32,
    /// Length of each command bar.
    pub length: f32,
    /// Width of each command bar.
    pub thickness: f32,
}

/// Draws valid attitude commands without creating a scene layer.
pub fn draw(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: DirectorLayout,
) -> Result<(), SceneError> {
    let Some(error) = attitude_error(data) else {
        return Ok(());
    };
    let pitch = (error.pitch_rad * RAD_TO_DEG * layout.pixels_per_degree[0])
        .clamp(-layout.travel, layout.travel);
    let bank = (error.roll_rad * RAD_TO_DEG * layout.pixels_per_degree[1])
        .clamp(-layout.travel, layout.travel);
    let [x, y] = layout.center;
    scene.fill_color(palette::MAGENTA)?;
    scene.rect(
        PaintMode::Fill,
        x - layout.length / 2.0,
        y - pitch - layout.thickness / 2.0,
        layout.length,
        layout.thickness,
    )?;
    scene.rect(
        PaintMode::Fill,
        x + bank - layout.thickness / 2.0,
        y - layout.length / 2.0,
        layout.thickness,
        layout.length,
    )
}

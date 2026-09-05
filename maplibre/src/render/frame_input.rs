//! Per-frame input from the host: the frame time and where the view comes from.
//!
//! A host writes the [`FrameInput`] resource before it runs the schedule, and the
//! [`frame_input_system`] at the head of the Extract stage applies it, ahead of the tile
//! request systems, so every system sees one consistent view state for the frame. Gesture handlers keep steering the map view
//! directly, since they are that view's source; an external view replaces it for the frame.

use std::time::Duration;

use crate::{
    context::MapContext,
    render::view_state::{ExternalView, ExternalViewError, ViewState},
    tcs::system::SystemResult,
};

/// Where the frame's view comes from.
#[derive(Clone, Copy, Debug, Default)]
pub enum ViewSource {
    /// The map's own camera, steered by the gesture handlers and the pose API.
    #[default]
    MapView,
    /// Matrices supplied by the host, for head tracking or one eye of a stereo pair.
    External(ExternalView),
}

/// What the host knows about the frame it is about to render.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameInput {
    /// Time since the host started; animated properties read it.
    pub timestamp: Duration,
    /// Where the frame's view comes from.
    pub view: ViewSource,
}

impl FrameInput {
    /// Moves the frame time forward, for hosts that count frames rather than read a clock.
    pub fn advance(&mut self, by: Duration) {
        self.timestamp = self.timestamp.saturating_add(by);
    }
}

/// Applies the frame input to the view state.
///
/// A map view keeps what the handlers set and drops any external projection. An external view
/// that cannot be applied leaves the view state as it was.
pub fn apply_frame_input(
    input: &FrameInput,
    view_state: &mut ViewState,
) -> Result<(), ExternalViewError> {
    match &input.view {
        ViewSource::MapView => {
            view_state.clear_external_view();
            Ok(())
        }
        ViewSource::External(external) => view_state.set_external_view(*external),
    }
}

/// Applies the [`FrameInput`] a host wrote; a host that writes none keeps the map view.
///
/// A rejected external view is logged and the frame falls back to the map view, so one bad
/// pose from a tracker never blanks the screen.
pub fn frame_input_system(
    MapContext {
        world, view_state, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(input) = world.resources.get::<FrameInput>() else {
        return Ok(());
    };
    if let Err(error) = apply_frame_input(input, view_state) {
        tracing::error!(%error, "external view rejected; the frame keeps the map view");
        view_state.clear_external_view();
    }
    Ok(())
}

#[cfg(test)]
mod tests;

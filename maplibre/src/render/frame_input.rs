//! Per-frame input from the host: the frame time and where the view comes from.
//!
//! A host writes the [`FrameInput`] resource before it runs the schedule, and the
//! [`frame_input_system`] at the head of the Extract stage applies it, ahead of the tile
//! request systems, so every system sees one consistent view state for the frame. Gesture handlers keep steering the map view
//! directly, since they are that view's source; an external view replaces it for the frame.

use std::time::Duration;

use thiserror::Error;

use crate::{
    context::MapContext,
    projection::ProjectionType,
    render::view_state::{ExternalView, ExternalViewError, ViewState},
    tcs::system::SystemResult,
};

/// Where the frame's view comes from.
#[derive(Clone, Copy, Debug, Default)]
pub enum ViewSource {
    /// The map's own camera, steered by the gesture handlers and the pose API.
    #[default]
    MapView,
    /// An eye supplied by the host, for head tracking or one eye of a stereo pair.
    External(ExternalView),
}

/// Why a frame's view cannot be applied.
#[derive(Error, Debug, Clone, Copy, PartialEq)]
pub enum FrameInputError {
    /// The external view was refused.
    #[error(transparent)]
    External(#[from] ExternalViewError),
}

/// What the host knows about the frame it is about to render.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    /// Time since the host started; animated properties read it.
    pub timestamp: Duration,
    /// Where the frame's view comes from.
    pub view: ViewSource,
    /// How far beyond an external eye's frustum tiles are requested, as a factor on its
    /// tangents: one requests what the frame shows, more keeps tiles ready for where a head
    /// may turn before they could load. Ignored for the map's own view.
    pub request_overscan: f64,
}

impl Default for FrameInput {
    fn default() -> Self {
        Self {
            timestamp: Duration::ZERO,
            view: ViewSource::MapView,
            request_overscan: 1.0,
        }
    }
}

impl FrameInput {
    /// Moves the frame time forward, for hosts that count frames rather than read a clock.
    pub fn advance(&mut self, by: Duration) {
        self.timestamp = self.timestamp.saturating_add(by);
    }
}

/// Applies the frame input to the view state, with the style's projection deciding whether
/// an external eye is placed on the globe or over the flat map.
///
/// A map view keeps what the handlers set and drops any external eye. An external view that
/// cannot be applied leaves the view state as it was.
pub fn apply_frame_input(
    input: &FrameInput,
    view_state: &mut ViewState,
    projection: &ProjectionType,
) -> Result<(), FrameInputError> {
    match &input.view {
        ViewSource::MapView => {
            view_state.clear_external_view();
            Ok(())
        }
        ViewSource::External(external) => {
            view_state.set_external_view(*external, projection)?;
            view_state.set_request_overscan(input.request_overscan);
            Ok(())
        }
    }
}

/// Applies the [`FrameInput`] a host wrote; a host that writes none keeps the map view.
///
/// A rejected external view is logged and the frame falls back to the map view, so one bad
/// pose from a tracker never blanks the screen.
pub fn frame_input_system(
    MapContext {
        world,
        view_state,
        style,
        ..
    }: &mut MapContext,
) -> SystemResult {
    super::eye_covering::snapshot_lod_history(world);
    let Some(input) = world.resources.get::<FrameInput>() else {
        return Ok(());
    };
    let projection = style
        .projection
        .as_ref()
        .map_or_else(ProjectionType::default, |specification| {
            specification.projection_type.clone()
        });
    if let Err(error) = apply_frame_input(input, view_state, &projection) {
        tracing::error!(%error, "host view rejected; the frame keeps the map view");
        view_state.clear_external_view();
    }
    Ok(())
}

#[cfg(test)]
mod tests;

//! Per-frame input from the host: the frame time and where the view comes from.
//!
//! A host writes the [`FrameInput`] resource before it runs the schedule, and the runtime
//! `frame_input_system` at the head of the Extract stage applies it, ahead of the tile request
//! systems, so every system sees one consistent view state for the frame. Gesture handlers keep
//! steering the map view directly, since they are that view's source; an external view replaces
//! it for the frame.

pub use crate::tcs::frame_input::{frame_time, FrameInput, FrameInputError, ViewSource};
use crate::{projection::ProjectionType, render::view_state::ViewState};

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

#[cfg(test)]
mod tests;

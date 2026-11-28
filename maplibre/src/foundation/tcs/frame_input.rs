//! Per-frame input types from the host: frame timestamp and view source.

use std::time::Duration;

use thiserror::Error;

use crate::{
    projection::camera_math::{ExternalView, ExternalViewError},
    tcs::world::World,
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

/// The time of the frame being rendered, as the host wrote it, for systems that animate or
/// pace their work; zero until the host writes its first [`FrameInput`].
pub fn frame_time(world: &World) -> Duration {
    world
        .resources
        .get::<FrameInput>()
        .map_or(Duration::ZERO, |input| input.timestamp)
}

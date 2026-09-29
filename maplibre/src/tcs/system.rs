//! Scheduling interface and typed system failures.

use std::borrow::Cow;

use thiserror::Error;

use crate::{context::MapContext, tcs::system::function::IntoSystem};

mod function;
pub mod heap;
pub mod stage;
pub mod timings;

/// Failure returned by a scheduled system; the stage stops before running later systems.
#[derive(Error, Debug)]
pub enum SystemError {
    /// Tile selection failed for the current view.
    #[error("failed to select tiles for the current view")]
    Projection(#[from] crate::render::projection::ProjectionStateError),
    /// Tile coordinates cannot be represented by the tile store.
    #[error("cannot request invalid tile {coords}")]
    InvalidTile {
        /// Rejected tile coordinates.
        coords: crate::coords::WorldTileCoords,
    },
    /// A worker did not accept a tile request; the tile remains eligible for retry.
    #[error("failed to submit {kind} tile request for {coords}")]
    TileRequest {
        /// Requested data family, such as vector, raster or DEM.
        kind: &'static str,
        /// Tile whose request was rejected.
        coords: crate::coords::WorldTileCoords,
        /// Underlying serialization or scheduling failure.
        #[source]
        source: crate::io::apc::CallError,
    },
    /// A worker result could not be interpreted by its receiving system.
    #[error("worker result has an invalid payload")]
    WorkerMessage(#[from] crate::io::apc::MessageError),
    /// An offscreen buffer could not be read back from the GPU.
    #[cfg(feature = "headless")]
    #[error("offscreen capture failed")]
    Capture(#[from] crate::render::resource::BufferReadbackError),
    /// An offscreen capture could not be encoded or written.
    #[cfg(feature = "headless")]
    #[error("writing offscreen image failed")]
    Image(#[from] crate::render::resource::WriteImageError),
    /// Renderer or GPU setup failed with a typed underlying cause.
    #[error("GPU operation failed")]
    Render(#[from] crate::render::error::RenderError),
    /// Render state could not be prepared for the requested operation.
    #[error("renderer was setup wrong")]
    Setup,
    /// A required resource is absent or has not been initialized.
    #[error("dependencies were not resolvable")]
    Dependencies,
}

/// Completion or failure of one scheduled system invocation.
pub type SystemResult = Result<(), SystemError>;

/// A map update that can be added to a [`Schedule`](crate::schedule::Schedule).
pub trait System: 'static {
    /// Returns the system's name.
    fn name(&self) -> Cow<'static, str>;

    /// Updates the map for the current frame. An error stops the stage without rolling back
    /// mutations made by this or earlier systems.
    fn run(&mut self, context: &mut MapContext) -> SystemResult;
}

/// A convenience type alias for a boxed [`System`] trait object.
pub type BoxedSystem = Box<dyn System>;

/// Owns a system whose concrete type is erased for storage in a stage.
pub struct SystemContainer {
    system: BoxedSystem,
}

impl SystemContainer {
    /// Boxes a system for execution in a stage.
    pub fn new<S: System>(system: S) -> Self {
        Self {
            system: Box::new(system),
        }
    }
}

/// Converts a system container or a stateful map-update function into an owned system.
pub trait IntoSystemContainer {
    /// Takes ownership of the update and erases its concrete type.
    fn into_container(self) -> SystemContainer;
}

impl<S> IntoSystemContainer for S
where
    S: IntoSystem,
{
    fn into_container(self) -> SystemContainer {
        SystemContainer {
            system: Box::new(IntoSystem::into_system(self)),
        }
    }
}

impl IntoSystemContainer for SystemContainer {
    fn into_container(self) -> SystemContainer {
        self
    }
}

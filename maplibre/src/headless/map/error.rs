//! Failures from headless data processing and frame execution.

use thiserror::Error;

use crate::{
    coords::WorldTileCoords, geojson::ProcessGeoJsonError, schedule::StageError,
    vector::ProcessVectorError,
};

/// Failure while processing or rendering data through a [`super::HeadlessMap`].
#[derive(Debug, Error)]
pub enum HeadlessMapOperationError {
    /// A processor returned a payload incompatible with its routing tag.
    #[error("headless worker result has an invalid payload")]
    WorkerMessage(#[from] crate::io::apc::MessageError),
    /// At least one frame is required for a render request.
    #[error("headless render frame count must be positive")]
    InvalidFrameCount,
    /// Tile coordinates cannot be represented by the tile store.
    #[error("cannot spawn headless tile {coords}")]
    InvalidTile {
        /// Invalid source-tile coordinates.
        coords: WorldTileCoords,
    },
    /// Vector source processing failed.
    #[error("headless vector source processing failed")]
    Vector {
        /// Underlying vector processor error.
        #[source]
        source: ProcessVectorError,
    },
    /// GeoJSON source processing failed.
    #[error("headless GeoJSON source processing failed")]
    GeoJson {
        /// Underlying GeoJSON processor error.
        #[source]
        source: ProcessGeoJsonError,
    },
    /// Render schedule execution failed.
    #[error("headless render schedule failed")]
    Schedule {
        /// Underlying schedule error.
        #[source]
        source: StageError,
    },
}

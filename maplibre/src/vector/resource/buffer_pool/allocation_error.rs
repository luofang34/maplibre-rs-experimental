use super::*;
/// Failure to fit one complete layer in the geometry pool.
#[derive(Debug, thiserror::Error)]
pub enum AllocationError {
    /// A buffer cannot contain the requested layer even when empty.
    #[error("{buffer:?} needs {requested} bytes but capacity is {capacity}")]
    Capacity {
        /// Backing buffer that cannot accept the payload.
        buffer: BackingBufferType,
        /// Requested byte count.
        requested: u64,
        /// Maximum byte count for the backing buffer.
        capacity: u64,
    },
    /// A vertex or feature payload violates GPU copy alignment.
    #[error("unaligned {buffer:?} payload: {bytes} bytes")]
    Alignment {
        /// Backing buffer that cannot accept the payload.
        buffer: BackingBufferType,
        /// Payload byte count.
        bytes: u64,
    },
    /// The tile cannot be addressed by the pool index.
    #[error("invalid tile coordinates {coords}")]
    Coordinates {
        /// Unaddressable tile.
        coords: WorldTileCoords,
    },
}

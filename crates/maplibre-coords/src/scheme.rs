//! Tile addressing conventions and tolerance comparison traits.

use serde::{Deserialize, Serialize};

/// Tiles can be positioned using either the xyz coordinates or the TMS (Tile Map Service) protocol.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileAddressingScheme {
    /// Rows start at the north edge of the world.
    #[serde(rename = "xyz")]
    #[default]
    XYZ,
    /// Rows start at the south edge; the tile loader flips the Y coordinate.
    #[serde(rename = "tms")]
    TMS,
}

/// Compares values using a caller-supplied tolerance rather than exact equality.
pub trait SignificantlyDifferent<Rhs: ?Sized = Self> {
    /// Tolerance type; its units and comparison semantics are defined by the implementation.
    type Epsilon;

    /// Whether the difference between these values exceeds the supplied tolerance.
    #[must_use]
    fn ne(&self, other: &Rhs, epsilon: Self::Epsilon) -> bool;
}

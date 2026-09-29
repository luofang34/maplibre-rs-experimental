//! Identity of an imagery source, independent from the tile coordinates it serves.

use serde::{Deserialize, Serialize};

/// Style source owning raster pixels. The fallback source is distinct from every named source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RasterSourceId(Option<String>);

impl RasterSourceId {
    /// Identifies a named style source, or the fallback source when no name is supplied.
    pub fn new(name: Option<String>) -> Self {
        Self(name)
    }

    /// Borrows the style source name; `None` identifies the fallback source, not a wildcard.
    pub fn name(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl From<String> for RasterSourceId {
    fn from(name: String) -> Self {
        Self(Some(name))
    }
}

impl From<&str> for RasterSourceId {
    fn from(name: &str) -> Self {
        Self(Some(name.to_owned()))
    }
}

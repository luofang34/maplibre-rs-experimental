//! Identity and sample revision of a decoded elevation tile.

use std::sync::Arc;

use super::DemTile;

/// A decoded DEM tile and the revision of its GPU-visible samples.
#[derive(Clone, Debug)]
pub struct LoadedDem {
    /// Decoded samples, shared copy-on-write across maps until neighbour borders diverge.
    /// In-place changes must use [`Arc::make_mut`] and advance `revision`.
    pub tile: Arc<DemTile>,
    /// Wrapping revision advanced whenever this tile's samples change.
    pub revision: u32,
    identity: Arc<()>,
}

impl LoadedDem {
    /// Wraps a freshly decoded tile with no neighbours filled in yet.
    pub fn new(tile: DemTile) -> Self {
        Self::from_shared(Arc::new(tile))
    }

    /// Wraps a shared immutable DEM tile so multiple maps reuse one decoded allocation.
    pub fn from_shared(tile: Arc<DemTile>) -> Self {
        Self {
            tile,
            revision: 0,
            identity: Arc::new(()),
        }
    }

    pub(crate) fn revision_key(&self) -> DemRevision {
        DemRevision {
            identity: self.identity.clone(),
            samples: self.revision,
        }
    }
}

// Caches retain the identity allocation so CPU eviction cannot reuse an old cache key.
#[derive(Clone, Debug)]
pub(crate) struct DemRevision {
    identity: Arc<()>,
    samples: u32,
}

impl PartialEq for DemRevision {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity) && self.samples == other.samples
    }
}

impl Eq for DemRevision {}

//! Identity and sample revision of a decoded elevation tile.

use std::sync::Arc;

use super::dem::DemTile;

/// A decoded DEM tile and the revision of its GPU-visible samples.
#[derive(Clone, Debug)]
pub struct LoadedDem {
    tile: Arc<DemTile>,
    /// Wrapping revision advanced whenever this tile's samples change.
    pub revision: u32,
    identity: Arc<()>,
}

impl std::ops::Deref for LoadedDem {
    type Target = DemTile;

    #[inline]
    fn deref(&self) -> &DemTile {
        &self.tile
    }
}

impl AsRef<Arc<DemTile>> for LoadedDem {
    #[inline]
    fn as_ref(&self) -> &Arc<DemTile> {
        &self.tile
    }
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

    pub(crate) fn fill_border_from_neighbour(
        &mut self,
        source_dim: u32,
        source_unpack: [f64; 4],
        (dx, dy): (i32, i32),
        samples: &[u8],
    ) -> Result<bool, super::dem::DemError> {
        self.tile.validate_neighbour(source_dim, source_unpack)?;
        if self.tile.border_matches(dx, dy, samples)? {
            return Ok(false);
        }
        let changed = Arc::make_mut(&mut self.tile).fill_border(dx, dy, samples)?;
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(changed)
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

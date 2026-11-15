//! Monotonic style revision tracking and cached worker snapshots.
//!
//! Each newly created or deserialized [`Style`] receives a distinct revision number from a
//! process-wide counter, and every style or source mutation bumps that revision while clearing
//! cached worker snapshots. Request systems and worker IPC reuse the cached `Arc<Style>` across
//! frames in O(1) without deep-cloning or re-serializing the full style document per frame.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use super::Style;
use crate::platform::lock_unpoisoned;

static NEXT_STYLE_REVISION: AtomicU64 = AtomicU64::new(1);

#[inline]
pub(crate) fn next_style_revision() -> u64 {
    NEXT_STYLE_REVISION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default, Debug)]
struct WorkerSnapshots {
    full: Option<Arc<Style>>,
    filtered: Option<((bool, bool), Arc<Style>)>,
    vector_layer_ids: Option<Arc<[String]>>,
    derived: Option<DerivedStyleFacts>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DerivedStyleFacts {
    pub depends_on_overscaling: bool,
    pub elevated_headroom_meters: f64,
}

/// Monotonic revision and lazy worker-snapshot cache owned by [`Style`].
#[derive(Debug)]
pub struct StyleRevisionState {
    revision: u64,
    snapshots: Mutex<WorkerSnapshots>,
}

impl Default for StyleRevisionState {
    fn default() -> Self {
        Self {
            revision: next_style_revision(),
            snapshots: Mutex::new(WorkerSnapshots::default()),
        }
    }
}

impl Clone for StyleRevisionState {
    fn clone(&self) -> Self {
        // A pure clone of an unchanged style preserves its revision identity while starting with
        // an empty snapshot slot so `Arc<Style>` snapshots never retain reference cycles.
        Self {
            revision: self.revision,
            snapshots: Mutex::new(WorkerSnapshots::default()),
        }
    }
}

impl StyleRevisionState {
    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn bump(&mut self) {
        self.revision = next_style_revision();
        if let Ok(guard) = self.snapshots.get_mut() {
            *guard = WorkerSnapshots::default();
        } else {
            *lock_unpoisoned(&self.snapshots) = WorkerSnapshots::default();
        }
    }
}

impl Style {
    /// Monotonic revision of this style document, bumped whenever layers, sources, images,
    /// transitions, feature-state, or global-state mutate.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision_state.revision()
    }

    /// Invalidates cached worker snapshots and advances [`Self::revision`] after an in-place edit.
    #[inline]
    pub fn bump_revision(&mut self) {
        self.revision_state.bump();
    }

    /// Returns a shared `Arc<Style>` snapshot for background tile workers, constructing it at most
    /// once per style revision.
    pub(crate) fn worker_snapshot(&self) -> Arc<Style> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(snapshot) = &guard.full {
            return snapshot.clone();
        }
        let snapshot = Arc::new(self.clone());
        guard.full = Some(snapshot.clone());
        snapshot
    }

    /// Returns a shared `Arc<Style>` snapshot filtered for `(vectors_disabled, symbols_disabled)`.
    pub(crate) fn filtered_worker_snapshot(
        &self,
        vectors_disabled: bool,
        symbols_disabled: bool,
    ) -> Arc<Style> {
        if !vectors_disabled && !symbols_disabled {
            return self.worker_snapshot();
        }
        let key = (vectors_disabled, symbols_disabled);
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some((cached_key, snapshot)) = &guard.filtered {
            if *cached_key == key {
                return snapshot.clone();
            }
        }
        let mut worker_style = self.clone();
        if symbols_disabled {
            worker_style.layers.retain(|layer| layer.type_ != "symbol");
        }
        if vectors_disabled {
            worker_style.layers.retain(|layer| layer.type_ == "symbol");
        }
        worker_style.bump_revision();
        let snapshot = Arc::new(worker_style);
        guard.filtered = Some((key, snapshot.clone()));
        snapshot
    }

    /// Cached ordered list of non-symbol vector layer IDs for the current style revision.
    pub(crate) fn cached_vector_layer_ids(&self) -> Arc<[String]> {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(ids) = &guard.vector_layer_ids {
            return ids.clone();
        }
        let ids: Arc<[String]> = self
            .layers
            .iter()
            .filter(|layer| layer.type_ != "symbol" && super::mutation::from_vector_tiles(layer))
            .map(|layer| layer.id.clone())
            .collect::<Vec<_>>()
            .into();
        guard.vector_layer_ids = Some(ids.clone());
        ids
    }

    /// Cached style-wide projection/overscaling facts recomputed only when [`Self::revision`] changes.
    pub(crate) fn cached_derived_facts(
        &self,
        compute: impl FnOnce(&Style) -> DerivedStyleFacts,
    ) -> DerivedStyleFacts {
        let mut guard = lock_unpoisoned(&self.revision_state.snapshots);
        if let Some(facts) = guard.derived {
            return facts;
        }
        let facts = compute(self);
        guard.derived = Some(facts);
        facts
    }
}

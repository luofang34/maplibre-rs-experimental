//! Cross-platform in-memory [`OfflineStore`] implementation.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;

use super::{
    region::OfflineRegionDefinition,
    store::{
        OfflineRegionId, OfflineRegionStatus, OfflineRegionSummary, OfflineStore, OfflineStoreError,
    },
};
use crate::io::source_client::ResourceKind;

struct MemRegion {
    definition: OfflineRegionDefinition,
    complete: bool,
    required_count: u64,
    urls: BTreeSet<String>,
}

struct MemResource {
    kind: ResourceKind,
    data: Vec<u8>,
    ambient: bool,
    last_access: u64,
    ref_count: usize,
}

struct MemoryState {
    next_id: i64,
    clock: u64,
    max_ambient_bytes: u64,
    tile_count_limit: Option<u64>,
    regions: BTreeMap<OfflineRegionId, MemRegion>,
    resources: HashMap<String, MemResource>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            next_id: 1,
            clock: 0,
            max_ambient_bytes: 32 * 1024 * 1024,
            tile_count_limit: None,
            regions: BTreeMap::new(),
            resources: HashMap::new(),
        }
    }
}

impl MemoryState {
    fn pinned_tile_count(&self) -> u64 {
        self.resources
            .values()
            .filter(|res| res.kind == ResourceKind::Tile && res.ref_count > 0)
            .count() as u64
    }

    fn region_status_sync(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionStatus, OfflineStoreError> {
        let region = self
            .regions
            .get(&id)
            .ok_or(OfflineStoreError::RegionNotFound(id))?;
        let mut bytes = 0_u64;
        let mut tiles = 0_u64;
        for url in &region.urls {
            if let Some(res) = self.resources.get(url) {
                bytes = bytes.saturating_add(res.data.len() as u64);
                if res.kind == ResourceKind::Tile {
                    tiles += 1;
                }
            }
        }
        Ok(OfflineRegionStatus {
            complete: region.complete,
            completed_resource_count: region.urls.len() as u64,
            completed_resource_bytes: bytes,
            completed_tile_count: tiles,
            required_resource_count: region.required_count,
        })
    }

    fn enforce_ambient_budget(&mut self) {
        loop {
            let unpinned_ambient_bytes: u64 = self
                .resources
                .values()
                .filter(|r| r.ref_count == 0 && r.ambient)
                .map(|r| r.data.len() as u64)
                .sum();
            if unpinned_ambient_bytes <= self.max_ambient_bytes {
                break;
            }
            let oldest = self
                .resources
                .iter()
                .filter(|(_, r)| r.ref_count == 0 && r.ambient)
                .min_by_key(|(_, r)| r.last_access)
                .map(|(url, _)| url.clone());
            match oldest {
                Some(url) => {
                    self.resources.remove(&url);
                }
                None => break,
            }
        }
    }
}

/// Cross-platform in-memory [`OfflineStore`] usable on both native and `wasm32` targets.
#[derive(Clone, Default)]
pub struct MemoryOfflineStore(Arc<Mutex<MemoryState>>);

impl MemoryOfflineStore {
    /// Creates an empty in-memory offline store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl OfflineStore for MemoryOfflineStore {
    async fn create_region(
        &self,
        definition: &OfflineRegionDefinition,
    ) -> Result<OfflineRegionId, OfflineStoreError> {
        let mut st = self.lock();
        let id = OfflineRegionId(st.next_id);
        st.next_id += 1;
        st.regions.insert(
            id,
            MemRegion {
                definition: definition.clone(),
                complete: false,
                required_count: 0,
                urls: BTreeSet::new(),
            },
        );
        Ok(id)
    }

    async fn list_regions(&self) -> Result<Vec<OfflineRegionSummary>, OfflineStoreError> {
        let st = self.lock();
        let mut out = Vec::with_capacity(st.regions.len());
        for (&id, region) in &st.regions {
            out.push(OfflineRegionSummary {
                id,
                definition: region.definition.clone(),
                status: st.region_status_sync(id)?,
            });
        }
        Ok(out)
    }

    async fn region_definition(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionDefinition, OfflineStoreError> {
        let st = self.lock();
        st.regions
            .get(&id)
            .map(|r| r.definition.clone())
            .ok_or(OfflineStoreError::RegionNotFound(id))
    }

    async fn region_status(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionStatus, OfflineStoreError> {
        self.lock().region_status_sync(id)
    }

    async fn set_region_required_count(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        let region = st
            .regions
            .get_mut(&id)
            .ok_or(OfflineStoreError::RegionNotFound(id))?;
        region.required_count = required_resource_count;
        Ok(())
    }

    async fn mark_region_complete(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        let region = st
            .regions
            .get_mut(&id)
            .ok_or(OfflineStoreError::RegionNotFound(id))?;
        region.required_count = required_resource_count;
        region.complete = true;
        Ok(())
    }

    async fn get_resource(&self, url: &str) -> Result<Option<Vec<u8>>, OfflineStoreError> {
        let mut st = self.lock();
        st.clock = st.clock.wrapping_add(1);
        let stamp = st.clock;
        Ok(st.resources.get_mut(url).map(|res| {
            res.last_access = stamp;
            res.data.clone()
        }))
    }

    async fn put_region_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        if !st.regions.contains_key(&region_id) {
            return Err(OfflineStoreError::RegionNotFound(region_id));
        }
        let becomes_newly_pinned_tile = kind == ResourceKind::Tile
            && st
                .resources
                .get(url)
                .is_none_or(|existing| existing.ref_count == 0);
        if becomes_newly_pinned_tile {
            if let Some(limit) = st.tile_count_limit {
                if st.pinned_tile_count() >= limit {
                    return Err(OfflineStoreError::TileLimitExceeded { limit });
                }
            }
        }
        st.clock = st.clock.wrapping_add(1);
        let stamp = st.clock;
        let is_new_link = st
            .regions
            .get_mut(&region_id)
            .is_some_and(|reg| reg.urls.insert(url.to_owned()));
        match st.resources.get_mut(url) {
            Some(existing) => {
                existing.data = data.to_vec();
                existing.kind = kind;
                existing.last_access = stamp;
                if is_new_link {
                    existing.ref_count += 1;
                }
            }
            None => {
                st.resources.insert(
                    url.to_owned(),
                    MemResource {
                        kind,
                        data: data.to_vec(),
                        ambient: false,
                        last_access: stamp,
                        ref_count: usize::from(is_new_link),
                    },
                );
            }
        }
        Ok(())
    }

    async fn link_existing_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
    ) -> Result<bool, OfflineStoreError> {
        let mut st = self.lock();
        if !st.regions.contains_key(&region_id) {
            return Err(OfflineStoreError::RegionNotFound(region_id));
        }
        let Some((kind, was_unpinned)) = st
            .resources
            .get(url)
            .map(|res| (res.kind, res.ref_count == 0))
        else {
            return Ok(false);
        };
        if kind == ResourceKind::Tile && was_unpinned {
            if let Some(limit) = st.tile_count_limit {
                if st.pinned_tile_count() >= limit {
                    return Err(OfflineStoreError::TileLimitExceeded { limit });
                }
            }
        }
        let added = st
            .regions
            .get_mut(&region_id)
            .is_some_and(|reg| reg.urls.insert(url.to_owned()));
        if added {
            if let Some(res) = st.resources.get_mut(url) {
                res.ref_count += 1;
            }
        }
        Ok(true)
    }

    async fn put_ambient_resource(
        &self,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        if st.max_ambient_bytes == 0 || (data.len() as u64) > st.max_ambient_bytes {
            return Ok(());
        }
        st.clock = st.clock.wrapping_add(1);
        let stamp = st.clock;
        match st.resources.get_mut(url) {
            Some(existing) => {
                existing.data = data.to_vec();
                existing.last_access = stamp;
            }
            None => {
                st.resources.insert(
                    url.to_owned(),
                    MemResource {
                        kind,
                        data: data.to_vec(),
                        ambient: true,
                        last_access: stamp,
                        ref_count: 0,
                    },
                );
            }
        }
        st.enforce_ambient_budget();
        Ok(())
    }

    async fn delete_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        let removed = st
            .regions
            .remove(&id)
            .ok_or(OfflineStoreError::RegionNotFound(id))?;
        for url in removed.urls {
            let drop_entry = if let Some(res) = st.resources.get_mut(&url) {
                res.ref_count = res.ref_count.saturating_sub(1);
                res.ref_count == 0 && !res.ambient
            } else {
                false
            };
            if drop_entry {
                st.resources.remove(&url);
            }
        }
        st.enforce_ambient_budget();
        Ok(())
    }

    async fn invalidate_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        let urls = {
            let reg = st
                .regions
                .get_mut(&id)
                .ok_or(OfflineStoreError::RegionNotFound(id))?;
            reg.complete = false;
            std::mem::take(&mut reg.urls)
        };
        for url in urls {
            let drop_entry = if let Some(res) = st.resources.get_mut(&url) {
                res.ref_count = res.ref_count.saturating_sub(1);
                res.ref_count == 0 && !res.ambient
            } else {
                false
            };
            if drop_entry {
                st.resources.remove(&url);
            }
        }
        st.enforce_ambient_budget();
        Ok(())
    }

    async fn set_maximum_ambient_bytes(&self, max_bytes: u64) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        st.max_ambient_bytes = max_bytes;
        st.enforce_ambient_budget();
        Ok(())
    }

    async fn set_tile_count_limit(&self, limit: Option<u64>) -> Result<(), OfflineStoreError> {
        let mut st = self.lock();
        st.tile_count_limit = limit;
        Ok(())
    }
}

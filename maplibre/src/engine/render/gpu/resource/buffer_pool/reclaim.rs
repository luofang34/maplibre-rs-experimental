//! Slab reclamation, tile/layer eviction helpers, and typed slab accessors.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct SharedLayerMetaAlloc {
    pub range: Range<wgpu::BufferAddress>,
    pub ref_count: u32,
    pub last_opaque: Option<bool>,
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Clears all resident geometry allocations and reclaims excess empty slabs.
    pub fn clear(&mut self) {
        self.index.clear();
        self.shared_vertices.clear();
        self.shared_layer_meta.clear();
        self.pattern_runs.clear();
        self.sort_runs.clear();
        for slab in self.vertex_slabs.iter_mut().flatten() {
            slab.clear();
        }
        for slab in self
            .index_u16_slabs
            .iter_mut()
            .flatten()
            .chain(self.index_native_slabs.iter_mut().flatten())
            .chain(self.layer_slabs.iter_mut().flatten())
            .chain(self.feature_slabs.iter_mut().flatten())
        {
            slab.clear();
        }
        self.reclaim_empty_pages(1);
        self.revision = self.revision.wrapping_add(1);
        self.eviction_generation = self.eviction_generation.wrapping_add(1);
    }

    /// Drops empty slabs beyond `keep_spare` per slab class when the pool owns a page factory.
    pub fn reclaim_empty_pages(&mut self, keep_spare: usize) {
        self.reclaim_empty_pages_with_queue(keep_spare, None);
    }

    /// Releases GPU geometry for a batch of already-evicted tiles in one `pattern_runs` /
    /// `sort_runs` retain pass without reclaiming slabs per tile.
    pub fn remove_evicted_tiles_batch(&mut self, evicted: &rustc_hash::FxHashSet<WorldTileCoords>) {
        if evicted.is_empty() {
            return;
        }
        self.pattern_runs
            .retain(|(tile, _), _| !evicted.contains(tile));
        self.sort_runs
            .retain(|(tile, _), _| !evicted.contains(tile));
        let mut freed_any = false;
        for &coords in evicted {
            let removed = self.index.remove_tile(coords);
            if !removed.is_empty() {
                freed_any = true;
                for entry in &removed {
                    self.free_entry_ranges(entry);
                }
            }
        }
        if freed_any {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Drops empty slabs beyond `keep_spare`, cancelling any staged writes on `queue` first.
    pub fn reclaim_empty_pages_with_queue(&mut self, keep_spare: usize, queue: Option<&Q>) {
        if self.page_factory.is_none() {
            return;
        }
        self.reclaim_empty_vertex_slabs(keep_spare, queue);
        Self::reclaim_slab_vec(
            &mut self.index_u16_slabs,
            keep_spare,
            queue,
            self.slab_destroyer.as_ref(),
        );
        Self::reclaim_slab_vec(
            &mut self.index_native_slabs,
            keep_spare,
            queue,
            self.slab_destroyer.as_ref(),
        );
        Self::reclaim_slab_vec(
            &mut self.layer_slabs,
            keep_spare,
            queue,
            self.slab_destroyer.as_ref(),
        );
        Self::reclaim_slab_vec(
            &mut self.feature_slabs,
            keep_spare,
            queue,
            self.slab_destroyer.as_ref(),
        );
    }

    pub(super) fn reclaim_empty_vertex_slabs(&mut self, keep_spare: usize, queue: Option<&Q>) {
        if self.page_factory.is_none() {
            return;
        }
        let mut spare_kept = 0usize;
        for slot in &mut self.vertex_slabs {
            if slot.as_ref().is_some_and(VertexSlab::is_empty) {
                if spare_kept < keep_spare {
                    spare_kept += 1;
                } else if let Some(slab) = slot.take() {
                    if let Some(q) = queue {
                        q.cancel_pending_writes(&slab.vertices.inner);
                        if let Some(fm) = &slab.paired_fm {
                            q.cancel_pending_writes(&fm.inner);
                        }
                    }
                    if let Some(destroyer) = &self.slab_destroyer {
                        (destroyer.0)(&slab.vertices.inner);
                        if let Some(fm) = &slab.paired_fm {
                            (destroyer.0)(&fm.inner);
                        }
                    }
                }
            }
        }
        while matches!(self.vertex_slabs.last(), Some(None)) {
            self.vertex_slabs.pop();
        }
    }

    pub(super) fn reclaim_slab_vec(
        slabs: &mut Vec<Option<BufferSlab<B>>>,
        keep_spare: usize,
        queue: Option<&Q>,
        destroyer: Option<&SlabDestroyer<B>>,
    ) {
        let mut spare_kept = 0usize;
        for slot in slabs.iter_mut() {
            if slot.as_ref().is_some_and(BufferSlab::is_empty) {
                if spare_kept < keep_spare {
                    spare_kept += 1;
                } else if let Some(slab) = slot.take() {
                    if let Some(q) = queue {
                        q.cancel_pending_writes(&slab.buffer.inner);
                    }
                    if let Some(d) = destroyer {
                        (d.0)(&slab.buffer.inner);
                    }
                }
            }
        }
        while matches!(slabs.last(), Some(None)) {
            slabs.pop();
        }
    }

    /// Backing vertex buffer of `entry`'s vertex slab.
    pub fn vertices_for(&self, entry: &IndexEntry) -> &B {
        &self.vertex_slab_of(entry.vertex_page).vertices.inner
    }

    /// Backing index buffer of `entry`'s index slab (`u16` or native `u32`).
    pub fn indices_for(&self, entry: &IndexEntry) -> &B {
        if entry.index_u16 {
            &self.index_u16_slab_of(entry.index_page).buffer.inner
        } else {
            &self.index_native_slab_of(entry.index_page).buffer.inner
        }
    }

    /// Backing layer-metadata buffer of `entry`'s layer slab.
    pub fn metadata_for(&self, entry: &IndexEntry) -> &B {
        &self.layer_slab_of(entry.layer_page).buffer.inner
    }

    /// Backing feature-metadata buffer of `entry` (either paired inside its vertex slab or standalone).
    pub fn feature_metadata_for(&self, entry: &IndexEntry) -> &B {
        match entry.fm_kind {
            FmStorageKind::Paired => {
                if let Some(fm) = &self.vertex_slab_of(entry.vertex_page).paired_fm {
                    &fm.inner
                } else {
                    &self.feature_slab_of(entry.feature_page).buffer.inner
                }
            }
            FmStorageKind::Standalone | FmStorageKind::None => {
                &self.feature_slab_of(entry.feature_page).buffer.inner
            }
        }
    }

    #[inline]
    pub(super) fn vertex_slab_of(&self, page: u16) -> &VertexSlab<B> {
        live_slab(&self.vertex_slabs, page)
    }

    #[inline]
    pub(super) fn index_u16_slab_of(&self, page: u16) -> &BufferSlab<B> {
        live_slab(&self.index_u16_slabs, page)
    }

    #[inline]
    pub(super) fn index_native_slab_of(&self, page: u16) -> &BufferSlab<B> {
        live_slab(&self.index_native_slabs, page)
    }

    #[inline]
    pub(super) fn layer_slab_of(&self, page: u16) -> &BufferSlab<B> {
        live_slab(&self.layer_slabs, page)
    }

    #[inline]
    pub(super) fn feature_slab_of(&self, page: u16) -> &BufferSlab<B> {
        live_slab(&self.feature_slabs, page)
    }

    pub(in crate::engine::render::gpu::resource::buffer_pool) fn release_shared_vertex_ref(
        &mut self,
        coords: WorldTileCoords,
        vertex_page: u16,
        shared_key: Option<u64>,
        family: u8,
    ) -> bool {
        let Some(key) = shared_key else {
            return true;
        };
        let map_key = (coords, vertex_page, key, family);
        match self.shared_vertices.get_mut(&map_key) {
            Some(shared) if shared.ref_count > 1 => {
                shared.ref_count -= 1;
                false
            }
            Some(_) => {
                self.shared_vertices.remove(&map_key);
                true
            }
            None => true,
        }
    }

    pub(in crate::engine::render::gpu::resource::buffer_pool) fn free_entry_ranges(
        &mut self,
        entry: &IndexEntry,
    ) {
        let release_vertices = self.release_shared_vertex_ref(
            entry.coords,
            entry.vertex_page,
            entry.shared_geometry_key,
            entry.shared_vertex_family,
        );
        if entry.vertex_page != NO_SLAB {
            if let Some(slab) = self
                .vertex_slabs
                .get_mut(usize::from(entry.vertex_page))
                .and_then(Option::as_mut)
            {
                if release_vertices {
                    slab.vertex_alloc.free(entry.buffer_vertices.clone());
                }
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.index_page != NO_SLAB {
            let slabs = if entry.index_u16 {
                &mut self.index_u16_slabs
            } else {
                &mut self.index_native_slabs
            };
            if let Some(slab) = slabs
                .get_mut(usize::from(entry.index_page))
                .and_then(Option::as_mut)
            {
                slab.alloc.free(entry.buffer_indices.clone());
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.layer_page != NO_SLAB {
            let release_layer_meta = self.release_shared_layer_meta_ref(
                entry.layer_page,
                entry.style_layer_id,
                u8::from(entry.coords.z),
            );
            if let Some(slab) = self
                .layer_slabs
                .get_mut(usize::from(entry.layer_page))
                .and_then(Option::as_mut)
            {
                if release_layer_meta {
                    slab.alloc.free(entry.buffer_layer_metadata.clone());
                }
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.fm_kind == FmStorageKind::Standalone && entry.feature_page != NO_SLAB {
            if let Some(slab) = self
                .feature_slabs
                .get_mut(usize::from(entry.feature_page))
                .and_then(Option::as_mut)
            {
                slab.alloc.free(entry.buffer_feature_metadata.clone());
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
    }

    /// Shares one `layer_metadata` record across all resident tiles of the same
    /// `(style_layer_id, tile_z)` so consecutive draws of the same style layer reuse one vertex
    /// buffer slice at Slot 2 across all vertex slabs.
    pub fn with_shared_layer_metadata(mut self) -> Self {
        self.shared_layer_metadata = true;
        self
    }

    #[inline]
    pub(super) fn layer_meta_share_key(
        &self,
        style_layer_id: crate::style::StyleLayerId,
        coords: WorldTileCoords,
    ) -> Option<(crate::style::StyleLayerId, u8)> {
        self.shared_layer_metadata
            .then_some((style_layer_id, u8::from(coords.z)))
    }

    pub(super) fn release_shared_layer_meta_ref(
        &mut self,
        layer_page: u16,
        style_layer_id: crate::style::StyleLayerId,
        tile_z: u8,
    ) -> bool {
        if !self.shared_layer_metadata {
            return true;
        }
        let map_key = (layer_page, style_layer_id, tile_z);
        match self.shared_layer_meta.get_mut(&map_key) {
            Some(shared) if shared.ref_count > 1 => {
                shared.ref_count -= 1;
                false
            }
            Some(_) => {
                self.shared_layer_meta.remove(&map_key);
                true
            }
            None => true,
        }
    }
}

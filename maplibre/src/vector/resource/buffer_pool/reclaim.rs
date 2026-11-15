//! Slab reclamation, tile/layer eviction helpers, and typed slab accessors.

use super::*;

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
    }

    /// Drops empty slabs beyond `keep_spare` per slab class when the pool owns a page factory.
    pub fn reclaim_empty_pages(&mut self, keep_spare: usize) {
        self.reclaim_empty_pages_with_queue(keep_spare, None);
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
}

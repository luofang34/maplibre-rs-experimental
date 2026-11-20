//! Per-class slab suballocation and lazy on-demand slab growth ramps.

use super::{page_elem_limit, *};

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    pub(super) fn try_alloc_or_grow_vertex(
        &mut self,
        queue: Option<&Q>,
        bytes: u64,
        align: u64,
        needs_paired_fm: bool,
        strides: ElementStrides,
    ) -> Option<(u16, Range<u64>)> {
        for (idx, slot) in self.vertex_slabs.iter_mut().enumerate() {
            let Some(slab) = slot.as_mut() else {
                continue;
            };
            if slab.paired_fm.is_some() != needs_paired_fm || slab.dedicated_oversize {
                continue;
            }
            if let Some(range) = slab.vertex_alloc.allocate(bytes, align) {
                slab.live_entries += 1;
                return Some((idx as u16, range));
            }
        }
        self.page_factory.as_ref()?;
        self.reclaim_empty_vertex_slabs(0, queue);
        if needs_paired_fm {
            Self::reclaim_slab_vec(
                &mut self.feature_slabs,
                0,
                queue,
                self.slab_destroyer.as_ref(),
            );
        }
        let used = self.allocated_elements();
        let req_elems = elem_div(bytes, strides.vertex).max(u64::from(bytes == 0));
        let rem_v = self.max_elements.vertices.saturating_sub(used.vertices);
        let rem_elems = if needs_paired_fm {
            rem_v.min(
                self.max_elements
                    .feature_metadata
                    .saturating_sub(used.feature_metadata),
            )
        } else {
            rem_v
        };
        if req_elems > rem_elems {
            return None;
        }
        let max_v_page = if needs_paired_fm {
            page_elem_limit(self.max_page_bytes, strides.vertex).min(page_elem_limit(
                self.max_page_bytes,
                strides.feature_metadata,
            ))
        } else {
            page_elem_limit(self.max_page_bytes, strides.vertex)
        };
        if req_elems > max_v_page {
            return None;
        }
        let is_oversize = req_elems > DEFAULT_PAGE_VERTICES;
        let slab_elems = if is_oversize {
            req_elems
                .next_multiple_of(1024)
                .min(max_v_page)
                .min(rem_elems)
                .max(req_elems)
        } else {
            let family_pages = self
                .vertex_slabs
                .iter()
                .filter(|slot| {
                    slot.as_ref().is_some_and(|slab| {
                        !slab.dedicated_oversize && slab.paired_fm.is_some() == needs_paired_fm
                    })
                })
                .count();
            let (target_v, req_align) = (
                match family_pages {
                    0 => 2 * 1024,
                    1 => 4 * 1024,
                    2 => 16 * 1024,
                    _ => DEFAULT_PAGE_VERTICES,
                },
                1024,
            );
            target_v
                .min(max_v_page)
                .max(req_elems.next_multiple_of(req_align).min(max_v_page))
                .min(rem_elems)
                .max(req_elems)
        };
        let v_bytes = fitting_buffer_size(strides.vertex as usize, slab_elems, self.max_page_bytes);
        let slot_idx = first_free_slot(&self.vertex_slabs);
        let page_id = u16::try_from(slot_idx).ok()?;
        let factory = self.page_factory.as_ref()?;
        let v_buf = (factory.0)(BackingBufferType::Vertices, v_bytes);
        let paired_desc = needs_paired_fm.then(|| {
            let fm_bytes = fitting_buffer_size(
                strides.feature_metadata as usize,
                slab_elems,
                self.max_page_bytes,
            );
            let fm_buf = (factory.0)(BackingBufferType::FeatureMetadata, fm_bytes);
            BackingBufferDescriptor::new(fm_buf, fm_bytes)
        });
        let mut slab = VertexSlab::new(
            BackingBufferDescriptor::new(v_buf, v_bytes),
            paired_desc,
            strides,
        );
        slab.dedicated_oversize = is_oversize;
        let range = slab.vertex_alloc.allocate(bytes, align)?;
        slab.live_entries = 1;
        place_slot(&mut self.vertex_slabs, slot_idx, slab);
        Some((page_id, range))
    }

    pub(super) fn try_alloc_or_grow_index(
        &mut self,
        queue: Option<&Q>,
        bytes: u64,
        use_u16: bool,
        strides: ElementStrides,
    ) -> Option<(u16, Range<u64>)> {
        let align = strides.active_index_align(use_u16);
        let elem_stride = strides.active_index_stride(use_u16);
        {
            let target_slabs = if use_u16 {
                &mut self.index_u16_slabs
            } else {
                &mut self.index_native_slabs
            };
            if bytes == 0 && target_slabs.iter().all(Option::is_none) && self.page_factory.is_some()
            {
                return Some((NO_SLAB, 0..0));
            }
            for (idx, slot) in target_slabs.iter_mut().enumerate() {
                if let Some(slab) = slot.as_mut() {
                    if !slab.dedicated_oversize {
                        if let Some(range) = slab.alloc.allocate(bytes, align) {
                            slab.live_entries += 1;
                            return Some((idx as u16, range));
                        }
                    }
                }
            }
        }
        self.page_factory.as_ref()?;
        Self::reclaim_slab_vec(
            &mut self.index_u16_slabs,
            0,
            queue,
            self.slab_destroyer.as_ref(),
        );
        Self::reclaim_slab_vec(
            &mut self.index_native_slabs,
            0,
            queue,
            self.slab_destroyer.as_ref(),
        );
        let rem_i = self
            .max_elements
            .indices
            .saturating_sub(self.allocated_elements().indices);
        let req_elems = elem_div(bytes, elem_stride).max(1);
        let max_i_page = page_elem_limit(self.max_page_bytes, elem_stride);
        if req_elems > rem_i || req_elems > max_i_page {
            return None;
        }
        let is_oversize = req_elems > DEFAULT_PAGE_INDICES;
        let slab_elems = if is_oversize {
            req_elems
                .next_multiple_of(1024)
                .min(max_i_page)
                .min(rem_i)
                .max(req_elems)
        } else {
            let existing_count = if use_u16 {
                self.index_u16_slabs
                    .iter()
                    .flatten()
                    .filter(|s| !s.dedicated_oversize)
                    .count()
            } else {
                self.index_native_slabs
                    .iter()
                    .flatten()
                    .filter(|s| !s.dedicated_oversize)
                    .count()
            };
            let (target_i, req_align) = (
                match existing_count {
                    0 => 4 * 1024,
                    1 => 16 * 1024,
                    2 => 32 * 1024,
                    _ => DEFAULT_PAGE_INDICES,
                },
                1024,
            );
            target_i
                .min(max_i_page)
                .max(req_elems.next_multiple_of(req_align).min(max_i_page))
                .min(rem_i)
                .max(req_elems)
        };
        let i_bytes = fitting_buffer_size(elem_stride as usize, slab_elems, self.max_page_bytes);
        let factory = self.page_factory.as_ref()?;
        let i_buf = (factory.0)(BackingBufferType::Indices, i_bytes);
        let mut slab = BufferSlab::new(BackingBufferDescriptor::new(i_buf, i_bytes), elem_stride);
        slab.dedicated_oversize = is_oversize;
        let range = slab.alloc.allocate(bytes, align)?;
        slab.live_entries = 1;
        let target_slabs = if use_u16 {
            &mut self.index_u16_slabs
        } else {
            &mut self.index_native_slabs
        };
        let slot_idx = first_free_slot(target_slabs);
        let page_id = u16::try_from(slot_idx).ok()?;
        place_slot(target_slabs, slot_idx, slab);
        Some((page_id, range))
    }

    pub(super) fn try_alloc_or_grow_layer(
        &mut self,
        queue: Option<&Q>,
        bytes: u64,
        strides: ElementStrides,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
    ) -> Option<(u16, Range<u64>)> {
        if strides.layer_metadata == 0 {
            return Some((NO_SLAB, 0..0));
        }
        if let Some((lid, z)) = layer_meta_key {
            for (idx, slot) in self.layer_slabs.iter_mut().enumerate() {
                let Some(slab) = slot.as_mut() else {
                    continue;
                };
                let page_id = idx as u16;
                if let Some(shared) = self.shared_layer_meta.get_mut(&(page_id, lid, z)) {
                    shared.ref_count += 1;
                    slab.live_entries += 1;
                    return Some((page_id, shared.range.clone()));
                }
            }
        }
        for (idx, slot) in self.layer_slabs.iter_mut().enumerate() {
            if let Some(slab) = slot.as_mut() {
                if let Some(range) = slab.alloc.allocate(bytes, strides.layer_align) {
                    slab.live_entries += 1;
                    let page_id = idx as u16;
                    if let Some((lid, z)) = layer_meta_key {
                        self.shared_layer_meta.insert(
                            (page_id, lid, z),
                            shared_layer_meta::SharedLayerMetaAlloc {
                                range: range.clone(),
                                ref_count: 1,
                            },
                        );
                    }
                    return Some((page_id, range));
                }
            }
        }
        self.page_factory.as_ref()?;
        Self::reclaim_slab_vec(
            &mut self.layer_slabs,
            0,
            queue,
            self.slab_destroyer.as_ref(),
        );
        let rem_lm = self
            .max_elements
            .layer_metadata
            .saturating_sub(self.allocated_elements().layer_metadata);
        let req_elems = elem_div(bytes, strides.layer_metadata).max(1);
        let max_lm_page = page_elem_limit(self.max_page_bytes, strides.layer_metadata);
        if req_elems > rem_lm || req_elems > max_lm_page {
            return None;
        }
        #[cfg(target_arch = "wasm32")]
        let initial_lm = 64;
        #[cfg(not(target_arch = "wasm32"))]
        let initial_lm = 256;
        let target_lm = if self.layer_slabs.iter().any(Option::is_some) {
            DEFAULT_PAGE_LAYER_META
        } else {
            initial_lm
        };
        let slab_elems = target_lm
            .min(max_lm_page)
            .max(req_elems.next_power_of_two().min(max_lm_page))
            .min(rem_lm)
            .max(req_elems);
        let lm_bytes = fitting_buffer_size(
            strides.layer_metadata as usize,
            slab_elems,
            self.max_page_bytes,
        );
        let slot_idx = first_free_slot(&self.layer_slabs);
        let page_id = u16::try_from(slot_idx).ok()?;
        let factory = self.page_factory.as_ref()?;
        let lm_buf = (factory.0)(BackingBufferType::Metadata, lm_bytes);
        let mut slab = BufferSlab::new(
            BackingBufferDescriptor::new(lm_buf, lm_bytes),
            strides.layer_metadata,
        );
        let range = slab.alloc.allocate(bytes, strides.layer_align)?;
        slab.live_entries = 1;
        place_slot(&mut self.layer_slabs, slot_idx, slab);
        if let Some((lid, z)) = layer_meta_key {
            self.shared_layer_meta.insert(
                (page_id, lid, z),
                shared_layer_meta::SharedLayerMetaAlloc {
                    range: range.clone(),
                    ref_count: 1,
                },
            );
        }
        Some((page_id, range))
    }

    pub(super) fn try_alloc_or_grow_feature(
        &mut self,
        queue: Option<&Q>,
        bytes: u64,
        strides: ElementStrides,
    ) -> Option<(u16, Range<u64>)> {
        for (idx, slot) in self.feature_slabs.iter_mut().enumerate() {
            if let Some(slab) = slot.as_mut() {
                if !slab.dedicated_oversize {
                    if let Some(range) = slab.alloc.allocate(bytes, strides.feature_align) {
                        slab.live_entries += 1;
                        return Some((idx as u16, range));
                    }
                }
            }
        }
        self.page_factory.as_ref()?;
        Self::reclaim_slab_vec(
            &mut self.feature_slabs,
            0,
            queue,
            self.slab_destroyer.as_ref(),
        );
        self.reclaim_empty_vertex_slabs(0, queue);
        let rem_fm = self
            .max_elements
            .feature_metadata
            .saturating_sub(self.allocated_elements().feature_metadata);
        let req_elems = elem_div(bytes, strides.feature_metadata).max(1);
        let max_fm_page = page_elem_limit(self.max_page_bytes, strides.feature_metadata);
        if req_elems > rem_fm || req_elems > max_fm_page {
            return None;
        }
        let is_oversize = req_elems > DEFAULT_PAGE_FEATURE_META;
        let slab_elems = if is_oversize {
            req_elems
                .next_multiple_of(1024)
                .min(max_fm_page)
                .min(rem_fm)
                .max(req_elems)
        } else {
            let count = self
                .feature_slabs
                .iter()
                .flatten()
                .filter(|s| !s.dedicated_oversize)
                .count();
            let (target_fm, min_req) = (
                match count {
                    0 => 2 * 1024,
                    1 => 8 * 1024,
                    _ => DEFAULT_PAGE_FEATURE_META,
                },
                req_elems.next_multiple_of(1024).min(max_fm_page),
            );
            target_fm
                .min(max_fm_page)
                .max(min_req)
                .min(rem_fm)
                .max(req_elems)
        };
        let fm_bytes = fitting_buffer_size(
            strides.feature_metadata as usize,
            slab_elems,
            self.max_page_bytes,
        );
        let slot_idx = first_free_slot(&self.feature_slabs);
        let page_id = u16::try_from(slot_idx).ok()?;
        let factory = self.page_factory.as_ref()?;
        let fm_buf = (factory.0)(BackingBufferType::FeatureMetadata, fm_bytes);
        let mut slab = BufferSlab::new(
            BackingBufferDescriptor::new(fm_buf, fm_bytes),
            strides.feature_metadata,
        );
        slab.dedicated_oversize = is_oversize;
        let range = slab.alloc.allocate(bytes, strides.feature_align)?;
        slab.live_entries = 1;
        place_slot(&mut self.feature_slabs, slot_idx, slab);
        Some((page_id, range))
    }
}

fn first_free_slot<T>(slabs: &[Option<T>]) -> usize {
    slabs
        .iter()
        .position(Option::is_none)
        .unwrap_or(slabs.len())
}

fn place_slot<T>(slabs: &mut Vec<Option<T>>, slot_idx: usize, slab: T) {
    if slot_idx == slabs.len() {
        slabs.push(Some(slab));
    } else {
        slabs[slot_idx] = Some(slab);
    }
}

fn elem_div(bytes: u64, stride: u64) -> u64 {
    bytes.checked_div(stride).unwrap_or(0)
}

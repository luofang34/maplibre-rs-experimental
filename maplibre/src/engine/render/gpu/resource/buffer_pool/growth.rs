use super::*;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    pub(super) fn allocated_elements(&self) -> BufferPoolSizes {
        let vertices = self
            .vertex_slabs
            .iter()
            .flatten()
            .map(|s| s.element_capacity)
            .sum();
        let indices = self
            .index_u16_slabs
            .iter()
            .flatten()
            .chain(self.index_native_slabs.iter().flatten())
            .map(|s| s.element_capacity)
            .sum();
        let layer_metadata = self
            .layer_slabs
            .iter()
            .flatten()
            .map(|s| s.element_capacity)
            .sum();
        let paired_fm: u64 = self
            .vertex_slabs
            .iter()
            .flatten()
            .filter(|s| s.paired_fm.is_some())
            .map(|s| s.element_capacity)
            .sum();
        let standalone_fm: u64 = self
            .feature_slabs
            .iter()
            .flatten()
            .map(|s| s.element_capacity)
            .sum();
        BufferPoolSizes {
            vertices,
            indices,
            layer_metadata,
            feature_metadata: paired_fm.saturating_add(standalone_fm),
        }
    }

    pub(super) fn single_layer_limits(
        &self,
        req: SlabRequest,
        strides: ElementStrides,
    ) -> [u64; 4] {
        let idx_stride = strides.active_index_stride(req.index_u16);
        let max_v_base = self
            .max_elements
            .vertices
            .min(page_elem_limit(self.max_page_bytes, strides.vertex));
        let (max_v_elems, max_fm_elems) = if req.fm_kind == FmStorageKind::Paired
            && strides.feature_metadata > 0
        {
            let clamped = max_v_base
                .min(self.max_elements.feature_metadata)
                .min(page_elem_limit(
                    self.max_page_bytes,
                    strides.feature_metadata,
                ));
            (clamped, clamped)
        } else {
            let max_fm = self.max_elements.feature_metadata.min(page_elem_limit(
                self.max_page_bytes,
                strides.feature_metadata,
            ));
            (max_v_base, max_fm)
        };
        let max_i_elems = self
            .max_elements
            .indices
            .min(page_elem_limit(self.max_page_bytes, idx_stride));
        let max_lm_elems = self
            .max_elements
            .layer_metadata
            .min(page_elem_limit(self.max_page_bytes, strides.layer_metadata));
        [
            max_v_elems * strides.vertex,
            max_i_elems * idx_stride,
            max_lm_elems * strides.layer_metadata,
            max_fm_elems * strides.feature_metadata,
        ]
    }

    pub(super) fn validate_request(
        &self,
        req: SlabRequest,
        strides: ElementStrides,
    ) -> Result<(), AllocationError> {
        let limits = self.single_layer_limits(req, strides);
        let sizes = [
            req.vertex_bytes,
            req.index_bytes,
            req.layer_bytes,
            req.feature_bytes,
        ];
        let types = [
            BackingBufferType::Vertices,
            BackingBufferType::Indices,
            BackingBufferType::Metadata,
            BackingBufferType::FeatureMetadata,
        ];
        for ((typ, bytes), capacity) in types.into_iter().zip(sizes).zip(limits) {
            if bytes > capacity {
                return Err(AllocationError::Capacity {
                    buffer: typ,
                    requested: bytes,
                    capacity,
                });
            }
            if !bytes.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
                return Err(AllocationError::Alignment { buffer: typ, bytes });
            }
        }
        Ok(())
    }

    pub(super) fn try_reserve_slabs(
        &mut self,
        queue: Option<&Q>,
        req: SlabRequest,
        strides: ElementStrides,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
        shared_vertex_slot: Option<(u16, Range<u64>)>,
    ) -> Option<SlabAllocation> {
        let needs_paired_fm = req.fm_kind == FmStorageKind::Paired && strides.feature_metadata > 0;
        let (vertex_page, vertices, release_v_on_abort) = if let Some((vp, vr)) = shared_vertex_slot
        {
            let slab = self.vertex_slabs.get_mut(usize::from(vp))?.as_mut()?;
            slab.live_entries += 1;
            (vp, vr, false)
        } else {
            let v_align = strides.vertex_byte_align(needs_paired_fm, req.vertex_elem_align);
            let (vp, vr) = self.try_alloc_or_grow_vertex(
                queue,
                req.vertex_bytes,
                v_align,
                needs_paired_fm,
                strides,
            )?;
            (vp, vr, true)
        };

        let paired_fm_range = if needs_paired_fm {
            let first_v = vertices.start.checked_div(strides.vertex).unwrap_or(0);
            let fm_start = first_v * strides.feature_metadata;
            fm_start..fm_start + req.feature_bytes
        } else {
            0..0
        };

        let Some((index_page, indices)) =
            self.try_alloc_or_grow_index(queue, req.index_bytes, req.index_u16, strides)
        else {
            self.abort_vertex(vertex_page, vertices, release_v_on_abort);
            return None;
        };

        let Some((layer_page, layer_metadata)) =
            self.try_alloc_or_grow_layer(queue, req.layer_bytes, strides, layer_meta_key)
        else {
            self.abort_vertex(vertex_page, vertices, release_v_on_abort);
            self.abort_index(index_page, indices, req.index_u16);
            return None;
        };

        let (feature_page, feature_metadata) = if req.fm_kind == FmStorageKind::Standalone
            && req.feature_bytes > 0
        {
            let Some((fp, fmr)) = self.try_alloc_or_grow_feature(queue, req.feature_bytes, strides)
            else {
                self.abort_vertex(vertex_page, vertices, release_v_on_abort);
                self.abort_index(index_page, indices, req.index_u16);
                self.abort_layer(layer_page, layer_metadata, layer_meta_key);
                return None;
            };
            (fp, fmr)
        } else if req.fm_kind == FmStorageKind::Paired {
            (NO_SLAB, paired_fm_range)
        } else {
            (NO_SLAB, 0..0)
        };

        Some(SlabAllocation {
            vertex_page,
            index_page,
            layer_page,
            feature_page,
            index_u16: req.index_u16,
            fm_kind: req.fm_kind,
            vertices,
            indices,
            layer_metadata,
            feature_metadata,
        })
    }

    pub(super) fn abort_allocation(
        &mut self,
        alloc: SlabAllocation,
        release_vertices: bool,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
    ) {
        self.abort_vertex(alloc.vertex_page, alloc.vertices, release_vertices);
        self.abort_index(alloc.index_page, alloc.indices, alloc.index_u16);
        self.abort_layer(alloc.layer_page, alloc.layer_metadata, layer_meta_key);
        if alloc.fm_kind == FmStorageKind::Standalone && alloc.feature_page != NO_SLAB {
            if let Some(slab) = self
                .feature_slabs
                .get_mut(usize::from(alloc.feature_page))
                .and_then(Option::as_mut)
            {
                slab.alloc.free(alloc.feature_metadata);
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
    }

    fn abort_vertex(&mut self, page: u16, range: Range<u64>, release_range: bool) {
        if page == NO_SLAB {
            return;
        }
        if let Some(slab) = self
            .vertex_slabs
            .get_mut(usize::from(page))
            .and_then(Option::as_mut)
        {
            if release_range {
                slab.vertex_alloc.free(range);
            }
            slab.live_entries = slab.live_entries.saturating_sub(1);
        }
    }

    fn abort_index(&mut self, page: u16, range: Range<u64>, use_u16: bool) {
        if page == NO_SLAB {
            return;
        }
        let slabs = if use_u16 {
            &mut self.index_u16_slabs
        } else {
            &mut self.index_native_slabs
        };
        if let Some(slab) = slabs.get_mut(usize::from(page)).and_then(Option::as_mut) {
            slab.alloc.free(range);
            slab.live_entries = slab.live_entries.saturating_sub(1);
        }
    }

    fn abort_layer(
        &mut self,
        page: u16,
        range: Range<u64>,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
    ) {
        if page == NO_SLAB {
            return;
        }
        let release_range = match layer_meta_key {
            Some((lid, z)) => self.release_shared_layer_meta_ref(page, lid, z),
            None => true,
        };
        if let Some(slab) = self
            .layer_slabs
            .get_mut(usize::from(page))
            .and_then(Option::as_mut)
        {
            if release_range {
                slab.alloc.free(range);
            }
            slab.live_entries = slab.live_entries.saturating_sub(1);
        }
    }

    pub(super) fn saturation_error(
        &self,
        req: SlabRequest,
        strides: ElementStrides,
    ) -> AllocationError {
        let limits = self.single_layer_limits(req, strides);
        AllocationError::Capacity {
            buffer: BackingBufferType::Vertices,
            requested: req.vertex_bytes,
            capacity: limits[0],
        }
    }

    pub(in crate::engine::render::gpu::resource::buffer_pool) fn reserve_allocation(
        &mut self,
        queue: Option<&Q>,
        req: SlabRequest,
        strides: ElementStrides,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
        shared_vertex_slot: Option<(u16, Range<u64>)>,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<SlabAllocation, AllocationError> {
        if let Some(alloc) = self.try_reserve_slabs(
            queue,
            req,
            strides,
            layer_meta_key,
            shared_vertex_slot.clone(),
        ) {
            return Ok(alloc);
        }

        let mut min_alloc_id = 0_u64;
        while let Some(evicted) = self.index.pop_oldest_from(&mut min_alloc_id, &mut protect) {
            self.pattern_runs
                .remove(&(evicted.coords, evicted.style_layer_id));
            self.sort_runs
                .remove(&(evicted.coords, evicted.style_layer_id));
            self.free_entry_ranges(&evicted);
            self.eviction_generation = self.eviction_generation.wrapping_add(1);

            if let Some(alloc) = self.try_reserve_slabs(
                queue,
                req,
                strides,
                layer_meta_key,
                shared_vertex_slot.clone(),
            ) {
                return Ok(alloc);
            }
        }

        Err(self.saturation_error(req, strides))
    }
}

pub(super) fn page_elem_limit(max_page_bytes: u64, stride: u64) -> u64 {
    max_page_bytes
        .checked_div(stride)
        .unwrap_or(0)
        .min(u64::from(u32::MAX))
}

mod slabs;

use super::*;

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    pub(super) fn single_layer_limits(&self, strides: ElementStrides) -> [u64; 4] {
        let max_v_elems = capped_elem_limit(
            self.max_capacities.vertices,
            self.max_page_bytes,
            strides.vertex,
        );
        let max_v_elems = match self
            .max_capacities
            .feature_metadata
            .checked_div(strides.feature_metadata)
            .zip(self.max_page_bytes.checked_div(strides.feature_metadata))
        {
            Some((fm_cap, fm_page)) => max_v_elems.min(fm_cap).min(fm_page),
            None => max_v_elems,
        };
        let max_i_elems = capped_elem_limit(
            self.max_capacities.indices,
            self.max_page_bytes,
            strides.index,
        );
        let max_lm_elems = capped_elem_limit(
            self.max_capacities.layer_metadata,
            self.max_page_bytes,
            strides.layer_metadata,
        );
        [
            max_v_elems * strides.vertex,
            max_i_elems * strides.index,
            max_lm_elems * strides.layer_metadata,
            max_v_elems * strides.feature_metadata,
        ]
    }

    pub(super) fn validate_sizes(
        &self,
        sizes: [u64; 4],
        strides: ElementStrides,
    ) -> Result<(), AllocationError> {
        let limits = self.single_layer_limits(strides);
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

    pub(super) fn try_grow_page_and_alloc(
        &mut self,
        sizes: [u64; 4],
        strides: ElementStrides,
    ) -> Option<PageAllocation> {
        self.page_factory.as_ref()?;
        // Any empty page reached here failed `try_allocate` in step 1 (too small for `sizes`), so
        // return its capacity to the unallocated pool before shaping a new page.
        self.reclaim_empty_pages(0);

        let rem = self
            .max_capacities
            .saturating_sub(self.allocated_capacities());
        let req_v_elems = elem_div(sizes[0], strides.vertex);
        let req_i_elems = elem_div(sizes[1], strides.index);
        let req_lm_elems = elem_div(sizes[2], strides.layer_metadata);

        let rem_v_base = elem_div(rem.vertices, strides.vertex);
        let rem_v_elems = match rem.feature_metadata.checked_div(strides.feature_metadata) {
            Some(rem_fm) => rem_v_base.min(rem_fm),
            None => rem_v_base,
        };
        let rem_i_elems = elem_div(rem.indices, strides.index);
        let rem_lm_elems = elem_div(rem.layer_metadata, strides.layer_metadata);

        if req_v_elems > rem_v_elems || req_i_elems > rem_i_elems || req_lm_elems > rem_lm_elems {
            return None;
        }

        let max_v_base = page_elem_limit(self.max_page_bytes, strides.vertex);
        let max_v_page = match self.max_page_bytes.checked_div(strides.feature_metadata) {
            Some(fm_page) => max_v_base.min(fm_page),
            None => max_v_base,
        };
        let max_i_page = page_elem_limit(self.max_page_bytes, strides.index);
        let max_lm_page = page_elem_limit(self.max_page_bytes, strides.layer_metadata);

        let total_v_elems = elem_div(self.max_capacities.vertices, strides.vertex).max(1);
        let total_i_elems = elem_div(self.max_capacities.indices, strides.index);
        let total_lm_elems = elem_div(self.max_capacities.layer_metadata, strides.layer_metadata);

        let page_v_elems = DEFAULT_PAGE_VERTICES
            .min(max_v_page)
            .max(req_v_elems.next_power_of_two().min(max_v_page))
            .min(rem_v_elems)
            .max(req_v_elems);
        let prop_i = (u128::from(total_i_elems) * u128::from(page_v_elems))
            .div_ceil(u128::from(total_v_elems)) as u64;
        let page_i_elems = prop_i
            .min(max_i_page)
            .max(req_i_elems.next_power_of_two().min(max_i_page))
            .min(rem_i_elems)
            .max(req_i_elems);
        let prop_lm = (u128::from(total_lm_elems) * u128::from(page_v_elems))
            .div_ceil(u128::from(total_v_elems)) as u64;
        let page_lm_elems = prop_lm
            .min(max_lm_page)
            .max(req_lm_elems.next_power_of_two().min(max_lm_page))
            .min(rem_lm_elems)
            .max(req_lm_elems);

        let v_bytes =
            fitting_buffer_size(strides.vertex as usize, page_v_elems, self.max_page_bytes);
        let i_bytes =
            fitting_buffer_size(strides.index as usize, page_i_elems, self.max_page_bytes);
        let lm_bytes = fitting_buffer_size(
            strides.layer_metadata as usize,
            page_lm_elems,
            self.max_page_bytes,
        );
        let fm_bytes = fitting_buffer_size(
            strides.feature_metadata as usize,
            page_v_elems,
            self.max_page_bytes,
        );

        let slot_idx = self
            .pages
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.pages.len());
        let page_id = u16::try_from(slot_idx).ok()?;

        let factory = self.page_factory.as_ref()?;
        let v_buf = (factory.0)(BackingBufferType::Vertices, v_bytes);
        let i_buf = (factory.0)(BackingBufferType::Indices, i_bytes);
        let lm_buf = (factory.0)(BackingBufferType::Metadata, lm_bytes);
        let fm_buf = (factory.0)(BackingBufferType::FeatureMetadata, fm_bytes);

        let mut page = BufferPage::new(
            BackingBufferDescriptor::new(v_buf, v_bytes),
            BackingBufferDescriptor::new(i_buf, i_bytes),
            BackingBufferDescriptor::new(lm_buf, lm_bytes),
            BackingBufferDescriptor::new(fm_buf, fm_bytes),
            strides,
        );
        let alloc = page.try_allocate(page_id, sizes, strides)?;
        if slot_idx == self.pages.len() {
            self.pages.push(Some(page));
        } else {
            self.pages[slot_idx] = Some(page);
        }
        Some(alloc)
    }

    pub(super) fn saturation_error(
        &self,
        sizes: [u64; 4],
        strides: ElementStrides,
    ) -> AllocationError {
        let limits = self.single_layer_limits(strides);
        let types = [
            BackingBufferType::Vertices,
            BackingBufferType::Indices,
            BackingBufferType::Metadata,
            BackingBufferType::FeatureMetadata,
        ];
        for ((typ, requested), capacity) in types.into_iter().zip(sizes).zip(limits) {
            let fits_somewhere = self.pages.iter().flatten().any(|page| match typ {
                BackingBufferType::Vertices => page
                    .vertex_alloc
                    .can_allocate(requested, strides.vertex_align),
                BackingBufferType::Indices => page
                    .index_alloc
                    .can_allocate(requested, strides.index_align),
                BackingBufferType::Metadata => page
                    .layer_alloc
                    .can_allocate(requested, strides.layer_align),
                BackingBufferType::FeatureMetadata => requested <= page.feature_metadata.inner_size,
            });
            if !fits_somewhere {
                return AllocationError::Capacity {
                    buffer: typ,
                    requested,
                    capacity,
                };
            }
        }
        AllocationError::Capacity {
            buffer: BackingBufferType::Vertices,
            requested: sizes[0],
            capacity: limits[0],
        }
    }
}

fn elem_div(bytes: u64, stride: u64) -> u64 {
    bytes.checked_div(stride).unwrap_or(0)
}

fn page_elem_limit(max_page_bytes: u64, stride: u64) -> u64 {
    max_page_bytes
        .checked_div(stride)
        .unwrap_or(0)
        .min(u64::from(u32::MAX))
}

fn capped_elem_limit(max_pool_bytes: u64, max_page_bytes: u64, stride: u64) -> u64 {
    elem_div(max_pool_bytes, stride).min(page_elem_limit(max_page_bytes, stride))
}

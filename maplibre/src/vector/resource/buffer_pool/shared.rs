//! Reference-counted shared vertex slab allocations and 16-byte uniform feature metadata uploads.

use super::{allocator::RangeAllocator, *};

#[derive(Clone, Debug)]
pub(super) struct SharedVertexAlloc {
    pub vertices: Range<wgpu::BufferAddress>,
    pub first_vertex: u32,
    pub ref_count: u32,
}

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Marks this pool to suballocate `feature_metadata` independently from `vertices`, allowing
    /// uniform-paint layers to reserve a single 16-byte `VertexStepMode::Instance` record and
    /// sibling layers on the same source layer to share a single vertex range.
    pub fn with_decoupled_feature_metadata(mut self) -> Self {
        self.decoupled_feature_metadata = true;
        let strides = ElementStrides::of::<V, I, TM, FM>();
        for page in self.pages.iter_mut().flatten() {
            page.decoupled_feature_metadata = true;
            if page.is_empty() {
                let usable_vertex_bytes = page
                    .vertices
                    .inner_size
                    .checked_div(strides.vertex)
                    .unwrap_or(0)
                    * strides.vertex;
                page.vertex_alloc = RangeAllocator::new(usable_vertex_bytes);
            }
        }
        self
    }

    /// Whether `feature_metadata` is suballocated independently rather than in vertex lockstep.
    #[inline]
    pub fn is_decoupled_feature_metadata(&self) -> bool {
        self.decoupled_feature_metadata
    }

    pub(super) fn release_shared_vertex_ref(
        &mut self,
        coords: WorldTileCoords,
        page: u16,
        shared_key: Option<u64>,
    ) -> bool {
        let Some(key) = shared_key else {
            return true;
        };
        let map_key = (coords, page, key);
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

    fn try_reuse_shared_page(
        &mut self,
        coords: WorldTileCoords,
        shared_key: u64,
        sizes_without_vertices: [u64; 4],
        strides: ElementStrides,
    ) -> Option<(PageAllocation, u32)> {
        for idx in 0..self.pages.len() {
            let page_id = idx as u16;
            if !self
                .shared_vertices
                .contains_key(&(coords, page_id, shared_key))
            {
                continue;
            }
            let alloc = self.pages[idx]
                .as_mut()
                .and_then(|page| page.try_allocate(page_id, sizes_without_vertices, strides));
            if let Some(mut alloc) = alloc {
                if let Some(shared) = self.shared_vertices.get_mut(&(coords, page_id, shared_key)) {
                    shared.ref_count += 1;
                    alloc.vertices = shared.vertices.clone();
                    return Some((alloc, shared.first_vertex));
                }
            }
        }
        None
    }

    /// Replaces `style_layer` at `coords` with uniform feature metadata, reusing an already-uploaded
    /// shared vertex buffer when `geometry.shared_geometry_key()` matches a resident sibling on the
    /// same page, or falling back to `fallback_vertices` when spilling onto a new page.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_layer_geometry_uniform_shared_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        fallback_vertices: Option<&[V]>,
        layer_metadata: TM,
        uniform_feature: FM,
        records: usize,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let strides = ElementStrides::of::<V, I, TM, FM>();
        let source_vertices: &[V] = if !geometry.buffer.vertices.is_empty() {
            &geometry.buffer.vertices
        } else {
            fallback_vertices.unwrap_or(&[])
        };
        let shared_key = geometry
            .shared_geometry_key()
            .filter(|_| self.decoupled_feature_metadata);
        let vertices = source_vertices.len() as u64;
        let records_u64 = records as u64;
        if !self.decoupled_feature_metadata && records_u64 > vertices {
            return Err(AllocationError::FeatureMetadata {
                vertices,
                records: records_u64,
            });
        }
        let uniform_fm = self.decoupled_feature_metadata;
        let fm_records = if uniform_fm {
            usize::from(
                strides.feature_metadata > 0 && (records > 0 || geometry.usable_indices > 0),
            )
        } else {
            records
        };
        let index_bytes = geometry.buffer.indices.len() as u64 * strides.index;
        let fm_bytes = (fm_records as u64) * strides.feature_metadata;

        let reused = if let Some(key) = shared_key {
            let reuse_sizes = [0, index_bytes, strides.layer_metadata, fm_bytes];
            self.validate_sizes(reuse_sizes, strides)?;
            self.try_reuse_shared_page(coords, key, reuse_sizes, strides)
        } else {
            None
        };

        let (alloc, first_vertex_u32, first_index_u32, uploaded_vertices) =
            if let Some((alloc, fv)) = reused {
                let first_index = alloc.indices.start.checked_div(strides.index).unwrap_or(0);
                let fi = match super::upload::element_index(first_index) {
                    Ok(fi) => fi,
                    Err(err) => {
                        let release_v =
                            self.release_shared_vertex_ref(coords, alloc.page, shared_key);
                        if let Some(page) = self
                            .pages
                            .get_mut(usize::from(alloc.page))
                            .and_then(Option::as_mut)
                        {
                            page.free(
                                alloc.vertices,
                                alloc.indices,
                                alloc.layer_metadata,
                                alloc.feature_metadata,
                                release_v,
                            );
                        }
                        return Err(err);
                    }
                };
                (alloc, fv, fi, false)
            } else {
                let sizes = [
                    vertices * strides.vertex,
                    index_bytes,
                    strides.layer_metadata,
                    fm_bytes,
                ];
                self.validate_sizes(sizes, strides)?;
                let alloc =
                    self.reserve_page_allocation(Some(queue), sizes, strides, &mut protect)?;
                let first_vertex = alloc
                    .vertices
                    .start
                    .checked_div(strides.vertex)
                    .unwrap_or(0);
                let first_index = alloc.indices.start.checked_div(strides.index).unwrap_or(0);
                let (fv, fi) = match (
                    super::upload::element_index(first_vertex),
                    super::upload::element_index(first_index),
                ) {
                    (Ok(fv), Ok(fi)) => (fv, fi),
                    (Err(err), _) | (_, Err(err)) => {
                        if let Some(page) = self
                            .pages
                            .get_mut(usize::from(alloc.page))
                            .and_then(Option::as_mut)
                        {
                            page.free(
                                alloc.vertices,
                                alloc.indices,
                                alloc.layer_metadata,
                                alloc.feature_metadata,
                                true,
                            );
                        }
                        return Err(err);
                    }
                };
                if let Some(key) = shared_key {
                    self.shared_vertices.insert(
                        (coords, alloc.page, key),
                        SharedVertexAlloc {
                            vertices: alloc.vertices.clone(),
                            first_vertex: fv,
                            ref_count: 1,
                        },
                    );
                }
                (alloc, fv, fi, vertices * strides.vertex > 0)
            };

        let style_layer_id = style_layer.layer_id();
        let allocation_id = self.revision.wrapping_add(1);
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            page: alloc.page,
            coords,
            style_layer_id,
            style_layer,
            usable_indices: geometry.usable_indices,
            first_vertex: first_vertex_u32,
            first_index: first_index_u32,
            buffer_vertices: alloc.vertices,
            buffer_indices: alloc.indices,
            buffer_layer_metadata: alloc.layer_metadata,
            buffer_feature_metadata: alloc.feature_metadata,
            uniform_feature_metadata: uniform_fm,
            shared_geometry_key: shared_key,
        };
        let page = self.page_of(&entry);
        if uploaded_vertices {
            queue.write_buffer(
                &page.vertices.inner,
                entry.buffer_vertices.start,
                bytemuck::cast_slice(source_vertices),
            );
        }
        if index_bytes > 0 {
            queue.write_buffer(
                &page.indices.inner,
                entry.buffer_indices.start,
                bytemuck::cast_slice(&geometry.buffer.indices),
            );
        }
        if strides.layer_metadata > 0 {
            queue.write_buffer(
                &page.layer_metadata.inner,
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&layer_metadata),
            );
        }
        if fm_bytes > 0 {
            Self::write_repeated_feature_metadata(
                queue,
                &page.feature_metadata.inner,
                entry.buffer_feature_metadata.start,
                uniform_feature,
                fm_records,
            );
        }
        self.index.push_back(entry);
        self.revision = allocation_id;
        let removed = self
            .index
            .remove_layer(coords, style_layer_id, Some(self.revision));
        for old in &removed {
            self.free_entry_ranges(old);
        }
        if !removed.is_empty() {
            self.reclaim_empty_pages_with_queue(1, Some(queue));
        }
        Ok(())
    }
}

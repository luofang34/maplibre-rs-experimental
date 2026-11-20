//! Reference-counted shared vertex & layer-metadata slab allocations and uniform uploads.

use super::{allocator::RangeAllocator, *};

#[derive(Clone, Debug)]
pub(super) struct SharedVertexAlloc {
    pub vertices: Range<wgpu::BufferAddress>,
    pub first_vertex: u32,
    pub vertex_count: u64,
    pub ref_count: u32,
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Marks this pool to suballocate `feature_metadata` independently from uniform `vertices`,
    /// allowing uniform-paint layers to reserve zero `feature_metadata` bytes (or a single
    /// instance-stepped record) and sibling layers on the same source layer to share vertices.
    pub fn with_decoupled_feature_metadata(mut self) -> Self {
        self.decoupled_feature_metadata = true;
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        for v_slab in self.vertex_slabs.iter_mut().flatten() {
            if v_slab.is_empty() {
                if let Some(paired) = v_slab.paired_fm.take() {
                    let desc = BackingBufferDescriptor::new(paired.inner, paired.inner_size);
                    self.feature_slabs
                        .push(Some(BufferSlab::new(desc, strides.feature_metadata)));
                }
                v_slab.element_capacity = v_slab
                    .vertices
                    .inner_size
                    .checked_div(strides.vertex)
                    .unwrap_or(0);
                v_slab.vertex_alloc = RangeAllocator::new(v_slab.element_capacity * strides.vertex);
            }
        }
        self
    }

    /// Whether `feature_metadata` can be suballocated independently rather than strictly in lockstep.
    #[inline]
    pub fn is_decoupled_feature_metadata(&self) -> bool {
        self.decoupled_feature_metadata
    }

    pub(super) fn release_shared_vertex_ref(
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

    fn find_shared_vertex_slot(
        &self,
        coords: WorldTileCoords,
        shared_key: u64,
        family: u8,
    ) -> Option<(u16, SharedVertexAlloc)> {
        for idx in 0..self.vertex_slabs.len() {
            let page_id = idx as u16;
            if let Some(shared) = self
                .shared_vertices
                .get(&(coords, page_id, shared_key, family))
            {
                return Some((page_id, shared.clone()));
            }
        }
        None
    }

    /// Returns `true` when `coords` holds a non-empty resident shared vertex slot for `(shared_key, layer_type)`.
    pub fn has_shared_vertex_slot(
        &self,
        coords: WorldTileCoords,
        shared_key: u64,
        layer_type: &str,
    ) -> bool {
        self.decoupled_feature_metadata
            && self
                .find_shared_vertex_slot(coords, shared_key, V::encoding_family(layer_type))
                .is_some_and(|(_, s)| s.vertex_count > 0)
    }

    /// Replaces `style_layer` at `coords` with uniform paint carried inside `layer_metadata` (zero
    /// `feature_metadata` buffer bytes reserved on decoupled pools), reusing shared sibling
    /// vertices when available.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_layer_geometry_layer_uniform_shared_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: impl Into<std::sync::Arc<StyleLayer>>,
        geometry: &OverAlignedVertexBuffer<V, I>,
        fallback_vertices: Option<&[V]>,
        layer_metadata: TM,
        uniform_feature: FM,
        records: usize,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_uniform_shared_inner(
            queue,
            coords,
            style_layer,
            geometry,
            fallback_vertices,
            layer_metadata,
            (uniform_feature, records, false),
            protect,
        )
    }

    /// Replaces `style_layer` at `coords` with uniform feature metadata, reusing an already-uploaded
    /// shared vertex buffer when `geometry.shared_geometry_key()` matches a resident sibling of the
    /// same encoding family, or falling back to `fallback_vertices` otherwise.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_layer_geometry_uniform_shared_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: impl Into<std::sync::Arc<StyleLayer>>,
        geometry: &OverAlignedVertexBuffer<V, I>,
        fallback_vertices: Option<&[V]>,
        layer_metadata: TM,
        uniform_feature: FM,
        records: usize,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_uniform_shared_inner(
            queue,
            coords,
            style_layer,
            geometry,
            fallback_vertices,
            layer_metadata,
            (uniform_feature, records, true),
            protect,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_uniform_shared_inner(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: impl Into<std::sync::Arc<StyleLayer>>,
        geometry: &OverAlignedVertexBuffer<V, I>,
        fallback_vertices: Option<&[V]>,
        layer_metadata: TM,
        (uniform_feature, records, needs_feature_slot): (FM, usize, bool),
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        let style_layer: std::sync::Arc<StyleLayer> = style_layer.into();
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let style_layer_id = style_layer.layer_id();
        let mut family = V::encoding_family(&style_layer.type_);
        let vertex_elem_align = V::vertex_element_alignment(&style_layer.type_);
        let layer_meta_key = self.layer_meta_share_key(style_layer_id, coords);
        let prepacked_info = Self::prepacked_info(geometry);
        let prepacked_verts = prepacked_info.map(|(v, _, cmp)| (v, cmp));
        let has_own_vertices = prepacked_verts.is_some() || !geometry.buffer.vertices.is_empty();
        let source_vertices: &[V] =
            if prepacked_verts.is_none() && !geometry.buffer.vertices.is_empty() {
                &geometry.buffer.vertices
            } else if prepacked_verts.is_none() {
                fallback_vertices.unwrap_or(&[])
            } else {
                &[]
            };
        let mut shared_key = geometry
            .shared_geometry_key()
            .filter(|_| self.decoupled_feature_metadata);

        let mut existing_shared =
            shared_key.and_then(|key| self.find_shared_vertex_slot(coords, key, family));
        if has_own_vertices && existing_shared.is_some() {
            tracing::warn!(
                %coords,
                layer = %style_layer.id,
                shared_key = shared_key.unwrap_or(0),
                "refusing to alias existing shared vertex slot for primary geometry upload"
            );
            existing_shared = None;
            shared_key = None;
        } else if let Some((_, ref shared_info)) = existing_shared {
            let fallback_mismatch = fallback_vertices
                .is_some_and(|verts| verts.len() as u64 != shared_info.vertex_count);
            if shared_info.vertex_count == 0 || fallback_mismatch {
                tracing::warn!(
                    %coords,
                    layer = %style_layer.id,
                    shared_key = shared_key.unwrap_or(0),
                    slot_vertices = shared_info.vertex_count,
                    "refusing mismatched or empty shared vertex slot"
                );
                existing_shared = None;
                shared_key = None;
            }
        }
        let vertices = match &existing_shared {
            Some((_, s)) => s.vertex_count,
            None => prepacked_info.map_or(source_vertices.len(), |(_, len, _)| len) as u64,
        };
        if existing_shared.is_none() && vertices == 0 {
            if geometry.usable_indices > 0 {
                return Err(AllocationError::MissingVertices {
                    coords,
                    usable_indices: geometry.usable_indices,
                });
            }
            shared_key = None;
        }
        let uniform_fm = self.decoupled_feature_metadata;
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let use_compact_extrusion = uniform_fm
            && !needs_feature_slot
            && strides.vertex == 16
            && existing_shared.is_none()
            && shared_key.is_none()
            && style_layer.type_ == "fill-extrusion"
            && (prepacked_info.is_some_and(|(_, _, cmp)| cmp)
                || (prepacked_verts.is_none()
                    && V::can_encode_compact_extrusion(&style_layer.type_, source_vertices)));
        let slab_vertex_words = if use_compact_extrusion {
            vertices.div_ceil(2)
        } else {
            vertices
        };
        if use_compact_extrusion {
            family = 4;
        }
        let records_u64 = records as u64;
        if !self.decoupled_feature_metadata && records_u64 > vertices {
            return Err(AllocationError::FeatureMetadata {
                vertices,
                records: records_u64,
            });
        }
        let fm_records = if uniform_fm {
            usize::from(
                needs_feature_slot
                    && strides.feature_metadata > 0
                    && (records > 0 || geometry.usable_indices > 0),
            )
        } else {
            records
        };
        let fm_bytes = (fm_records as u64) * strides.feature_metadata;
        let fm_kind = if !uniform_fm {
            if strides.feature_metadata > 0 {
                FmStorageKind::Paired
            } else {
                FmStorageKind::None
            }
        } else if fm_bytes > 0 {
            FmStorageKind::Standalone
        } else {
            FmStorageKind::None
        };

        let allow_chunks = u16_chunks::layer_allows_u16_chunks(&style_layer);
        let (index_u16, index_bytes, u16_chunk_ranges) =
            self.plan_geometry_index_upload(geometry, vertices, strides, allow_chunks);

        let (alloc, first_vertex_u32, first_index_u32, uploaded_vertices) =
            if let Some((vp, shared_info)) = existing_shared {
                let req = SlabRequest {
                    vertex_bytes: 0,
                    vertex_elem_align,
                    index_bytes,
                    index_u16,
                    layer_bytes: strides.layer_metadata,
                    feature_bytes: fm_bytes,
                    fm_kind,
                };
                self.validate_request(req, strides)?;
                let alloc = self.reserve_allocation(
                    Some(queue),
                    req,
                    strides,
                    layer_meta_key,
                    Some((vp, shared_info.vertices.clone())),
                    |c| c == coords || protect(c),
                )?;
                let idx_stride = strides.active_index_stride(alloc.index_u16);
                let first_index = alloc.indices.start.checked_div(idx_stride).unwrap_or(0);
                let fi = match super::upload::element_index(first_index) {
                    Ok(fi) => fi,
                    Err(err) => {
                        self.abort_allocation(alloc, false, layer_meta_key);
                        return Err(err);
                    }
                };
                if let Some(key) = shared_key {
                    if let Some(s) = self.shared_vertices.get_mut(&(coords, vp, key, family)) {
                        s.ref_count += 1;
                    }
                }
                (alloc, shared_info.first_vertex, fi, false)
            } else {
                let req = SlabRequest {
                    vertex_bytes: slab_vertex_words * strides.vertex,
                    vertex_elem_align,
                    index_bytes,
                    index_u16,
                    layer_bytes: strides.layer_metadata,
                    feature_bytes: fm_bytes,
                    fm_kind,
                };
                self.validate_request(req, strides)?;
                let alloc = self.reserve_allocation(
                    Some(queue),
                    req,
                    strides,
                    layer_meta_key,
                    None,
                    &mut protect,
                )?;
                let idx_stride = strides.active_index_stride(alloc.index_u16);
                let first_vert_stride = if use_compact_extrusion {
                    8
                } else {
                    strides.vertex
                };
                let first_vertex = alloc
                    .vertices
                    .start
                    .checked_div(first_vert_stride)
                    .unwrap_or(0);
                let first_index = alloc.indices.start.checked_div(idx_stride).unwrap_or(0);
                let (fv, fi) = match (
                    super::upload::element_index(first_vertex),
                    super::upload::element_index(first_index),
                ) {
                    (Ok(fv), Ok(fi)) => (fv, fi),
                    (Err(err), _) | (_, Err(err)) => {
                        self.abort_allocation(alloc, true, layer_meta_key);
                        return Err(err);
                    }
                };
                if let Some(key) = shared_key {
                    self.shared_vertices.insert(
                        (coords, alloc.vertex_page, key, family),
                        SharedVertexAlloc {
                            vertices: alloc.vertices.clone(),
                            first_vertex: fv,
                            vertex_count: vertices,
                            ref_count: 1,
                        },
                    );
                }
                (alloc, fv, fi, slab_vertex_words * strides.vertex > 0)
            };

        let allocation_id = self.revision.wrapping_add(1);
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            vertex_page: alloc.vertex_page,
            index_page: alloc.index_page,
            layer_page: alloc.layer_page,
            feature_page: alloc.feature_page,
            index_u16: alloc.index_u16,
            fm_kind: alloc.fm_kind,
            shared_vertex_family: family,
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
            dual_feature_metadata: false,
            dynamic_circle_metadata: false,
            shared_geometry_key: shared_key,
            evaluated_zoom: None,
            u16_chunks: u16_chunk_ranges,
            compact_extrusion: use_compact_extrusion,
            opaque_color: false,
        };

        if uploaded_vertices {
            self.write_geometry_vertices(
                queue,
                &entry,
                prepacked_verts,
                vertices as usize,
                source_vertices,
            );
        }
        self.write_geometry_indices(queue, &entry, geometry, index_bytes);
        if strides.layer_metadata > 0 && entry.layer_page != NO_SLAB {
            queue.write_buffer(
                self.metadata_for(&entry),
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&layer_metadata),
            );
        }
        if fm_bytes > 0 {
            let skip_zero_fill = if entry.fm_kind == FmStorageKind::Paired {
                let is_zero = self.zero_initialized_slabs
                    && bytemuck::bytes_of(&uniform_feature).iter().all(|&b| b == 0);
                if let Some(v_slab) = self
                    .vertex_slabs
                    .get_mut(usize::from(entry.vertex_page))
                    .and_then(Option::as_mut)
                {
                    let untouched =
                        entry.buffer_feature_metadata.start >= v_slab.paired_fm_dirty_end;
                    v_slab.paired_fm_dirty_end = v_slab
                        .paired_fm_dirty_end
                        .max(entry.buffer_feature_metadata.end);
                    is_zero && untouched
                } else {
                    false
                }
            } else {
                false
            };
            if !skip_zero_fill {
                Self::write_repeated_feature_metadata(
                    queue,
                    self.feature_metadata_for(&entry),
                    entry.buffer_feature_metadata.start,
                    uniform_feature,
                    fm_records,
                );
            }
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

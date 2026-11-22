use super::{SharedVertexAlloc, *};
use crate::vector::resource::buffer_pool::upload::UniformLayerUpload;

pub(super) struct SharedUniformTarget<'a, V> {
    pub fallback_vertices: Option<&'a [V]>,
    pub needs_feature_slot: bool,
}

pub(super) struct ResolvedVertexSource<'a, V> {
    pub family: u8,
    pub shared_key: Option<u64>,
    pub existing_shared: Option<(u16, SharedVertexAlloc)>,
    pub source_vertices: &'a [V],
    pub prepacked_verts: Option<(&'a [crate::render::shaders::GpuShaderVertex], bool)>,
    pub vertices: u64,
    pub compact_prepacked: bool,
}

pub(super) struct UniformSlabPlan {
    pub strides: ElementStrides,
    pub family: u8,
    pub use_compact_extrusion: bool,
    pub slab_vertex_words: u64,
    pub fm_records: usize,
    pub fm_bytes: u64,
    pub fm_kind: FmStorageKind,
    pub index_u16: bool,
    pub index_bytes: u64,
    pub u16_chunk_ranges: Option<u16_chunks::U16Chunks>,
    pub layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
}

pub(super) struct ReservedUniformSlot {
    pub alloc: SlabAllocation,
    pub first_vertex: u32,
    pub first_index: u32,
    pub uploaded_vertices: bool,
}

struct ExistingSharedSlotReq<'a> {
    coords: WorldTileCoords,
    vertex_elem_align: wgpu::BufferAddress,
    vp: u16,
    shared_info: &'a SharedVertexAlloc,
    shared_key: Option<u64>,
    plan: &'a UniformSlabPlan,
}

fn compute_fm_layout(
    uniform_fm: bool,
    fm_stride: u64,
    needs_feature_slot: bool,
    records: usize,
    has_usable_indices: bool,
) -> (usize, u64, FmStorageKind) {
    let fm_records = if uniform_fm {
        usize::from(needs_feature_slot && fm_stride > 0 && (records > 0 || has_usable_indices))
    } else {
        records
    };
    let fm_bytes = (fm_records as u64) * fm_stride;
    let fm_kind = if !uniform_fm {
        if fm_stride > 0 {
            FmStorageKind::Paired
        } else {
            FmStorageKind::None
        }
    } else if fm_bytes > 0 {
        FmStorageKind::Standalone
    } else {
        FmStorageKind::None
    };
    (fm_records, fm_bytes, fm_kind)
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    pub(super) fn resolve_shared_vertex_source<'a>(
        &self,
        coords: WorldTileCoords,
        style_layer: &StyleLayer,
        geometry: &'a OverAlignedVertexBuffer<V, I>,
        fallback_vertices: Option<&'a [V]>,
    ) -> Result<ResolvedVertexSource<'a, V>, AllocationError> {
        let family = V::encoding_family(&style_layer.type_);
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
            let mismatch = fallback_vertices
                .is_some_and(|verts| verts.len() as u64 != shared_info.vertex_count);
            if shared_info.vertex_count == 0 || mismatch {
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
        Ok(ResolvedVertexSource {
            family,
            shared_key,
            existing_shared,
            source_vertices,
            prepacked_verts,
            vertices,
            compact_prepacked: prepacked_info.is_some_and(|(_, _, cmp)| cmp),
        })
    }

    pub(super) fn plan_uniform_slab_request(
        &self,
        coords: WorldTileCoords,
        upload: &UniformLayerUpload<'_, TM, FM, V, I>,
        source: &ResolvedVertexSource<'_, V>,
        needs_feature_slot: bool,
    ) -> Result<UniformSlabPlan, AllocationError> {
        let uniform_fm = self.decoupled_feature_metadata;
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let use_compact_extrusion = uniform_fm
            && !needs_feature_slot
            && strides.vertex == 16
            && source.existing_shared.is_none()
            && source.shared_key.is_none()
            && upload.style_layer.type_ == "fill-extrusion"
            && (source.compact_prepacked
                || (source.prepacked_verts.is_none()
                    && V::can_encode_compact_extrusion(
                        &upload.style_layer.type_,
                        source.source_vertices,
                    )));
        let slab_vertex_words = if use_compact_extrusion {
            source.vertices.div_ceil(2)
        } else {
            source.vertices
        };
        let family = if use_compact_extrusion {
            4
        } else {
            source.family
        };
        let records_u64 = upload.records as u64;
        if !self.decoupled_feature_metadata && records_u64 > source.vertices {
            return Err(AllocationError::FeatureMetadata {
                vertices: source.vertices,
                records: records_u64,
            });
        }
        let (fm_records, fm_bytes, fm_kind) = compute_fm_layout(
            uniform_fm,
            strides.feature_metadata,
            needs_feature_slot,
            upload.records,
            upload.geometry.usable_indices > 0,
        );
        let allow_chunks = u16_chunks::layer_allows_u16_chunks(&upload.style_layer);
        let (index_u16, index_bytes, u16_chunk_ranges) = self.plan_geometry_index_upload(
            upload.geometry,
            source.vertices,
            strides,
            allow_chunks,
        );
        let layer_meta_key = self.layer_meta_share_key(upload.style_layer.layer_id(), coords);
        Ok(UniformSlabPlan {
            strides,
            family,
            use_compact_extrusion,
            slab_vertex_words,
            fm_records,
            fm_bytes,
            fm_kind,
            index_u16,
            index_bytes,
            u16_chunk_ranges,
            layer_meta_key,
        })
    }

    pub(super) fn reserve_shared_uniform_slabs(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_type: &str,
        source: &ResolvedVertexSource<'_, V>,
        plan: &UniformSlabPlan,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<ReservedUniformSlot, AllocationError> {
        let vertex_elem_align = V::vertex_element_alignment(style_type);
        if let Some((vp, ref shared_info)) = source.existing_shared {
            return self.reserve_existing_shared_slot(
                queue,
                ExistingSharedSlotReq {
                    coords,
                    vertex_elem_align,
                    vp,
                    shared_info,
                    shared_key: source.shared_key,
                    plan,
                },
                protect,
            );
        }
        self.reserve_fresh_uniform_slot(queue, coords, vertex_elem_align, source, plan, protect)
    }

    fn reserve_existing_shared_slot(
        &mut self,
        queue: &Q,
        req_info: ExistingSharedSlotReq<'_>,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<ReservedUniformSlot, AllocationError> {
        let ExistingSharedSlotReq {
            coords,
            vertex_elem_align,
            vp,
            shared_info,
            shared_key,
            plan,
        } = req_info;
        let req = SlabRequest {
            vertex_bytes: 0,
            vertex_elem_align,
            index_bytes: plan.index_bytes,
            index_u16: plan.index_u16,
            layer_bytes: plan.strides.layer_metadata,
            feature_bytes: plan.fm_bytes,
            fm_kind: plan.fm_kind,
        };
        self.validate_request(req, plan.strides)?;
        let alloc = self.reserve_allocation(
            Some(queue),
            req,
            plan.strides,
            plan.layer_meta_key,
            Some((vp, shared_info.vertices.clone())),
            |c| c == coords || protect(c),
        )?;
        let idx_stride = plan.strides.active_index_stride(alloc.index_u16);
        let first_index = alloc.indices.start.checked_div(idx_stride).unwrap_or(0);
        let fi = match super::upload::element_index(first_index) {
            Ok(fi) => fi,
            Err(err) => {
                self.abort_allocation(alloc, false, plan.layer_meta_key);
                return Err(err);
            }
        };
        if let Some(key) = shared_key {
            if let Some(s) = self
                .shared_vertices
                .get_mut(&(coords, vp, key, plan.family))
            {
                s.ref_count += 1;
            }
        }
        Ok(ReservedUniformSlot {
            alloc,
            first_vertex: shared_info.first_vertex,
            first_index: fi,
            uploaded_vertices: false,
        })
    }

    fn reserve_fresh_uniform_slot(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        vertex_elem_align: wgpu::BufferAddress,
        source: &ResolvedVertexSource<'_, V>,
        plan: &UniformSlabPlan,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<ReservedUniformSlot, AllocationError> {
        let req = SlabRequest {
            vertex_bytes: plan.slab_vertex_words * plan.strides.vertex,
            vertex_elem_align,
            index_bytes: plan.index_bytes,
            index_u16: plan.index_u16,
            layer_bytes: plan.strides.layer_metadata,
            feature_bytes: plan.fm_bytes,
            fm_kind: plan.fm_kind,
        };
        self.validate_request(req, plan.strides)?;
        let alloc = self.reserve_allocation(
            Some(queue),
            req,
            plan.strides,
            plan.layer_meta_key,
            None,
            &mut protect,
        )?;
        let idx_stride = plan.strides.active_index_stride(alloc.index_u16);
        let first_vert_stride = if plan.use_compact_extrusion {
            8
        } else {
            plan.strides.vertex
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
                self.abort_allocation(alloc, true, plan.layer_meta_key);
                return Err(err);
            }
        };
        if let Some(key) = source.shared_key {
            self.shared_vertices.insert(
                (coords, alloc.vertex_page, key, plan.family),
                SharedVertexAlloc {
                    vertices: alloc.vertices.clone(),
                    first_vertex: fv,
                    vertex_count: source.vertices,
                    ref_count: 1,
                },
            );
        }
        Ok(ReservedUniformSlot {
            alloc,
            first_vertex: fv,
            first_index: fi,
            uploaded_vertices: plan.slab_vertex_words * plan.strides.vertex > 0,
        })
    }

    pub(super) fn write_uniform_slot_buffers(
        &mut self,
        queue: &Q,
        entry: &IndexEntry,
        upload: &UniformLayerUpload<'_, TM, FM, V, I>,
        source: &ResolvedVertexSource<'_, V>,
        plan: &UniformSlabPlan,
        uploaded_vertices: bool,
    ) {
        if uploaded_vertices {
            self.write_geometry_vertices(
                queue,
                entry,
                source.prepacked_verts,
                source.vertices as usize,
                source.source_vertices,
            );
        }
        self.write_geometry_indices(queue, entry, upload.geometry, plan.index_bytes);
        if plan.strides.layer_metadata > 0 && entry.layer_page != NO_SLAB {
            queue.write_buffer(
                self.metadata_for(entry),
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&upload.layer_metadata),
            );
        }
        if plan.fm_bytes > 0 {
            let skip_zero_fill = if entry.fm_kind == FmStorageKind::Paired {
                let is_zero = self.zero_initialized_slabs
                    && bytemuck::bytes_of(&upload.uniform_feature)
                        .iter()
                        .all(|&b| b == 0);
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
                    self.feature_metadata_for(entry),
                    entry.buffer_feature_metadata.start,
                    upload.uniform_feature,
                    plan.fm_records,
                );
            }
        }
    }
}

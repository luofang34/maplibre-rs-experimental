//! Reference-counted shared vertex & layer-metadata slab allocations and uniform uploads.

use super::{allocator::RangeAllocator, upload::UniformLayerUpload, *};

mod uniform_slot;
use uniform_slot::SharedUniformTarget;

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
    pub fn replace_layer_geometry_layer_uniform_shared_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: UniformLayerUpload<'_, TM, FM, V, I>,
        fallback_vertices: Option<&[V]>,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_uniform_shared_inner(
            queue,
            coords,
            upload,
            SharedUniformTarget {
                fallback_vertices,
                needs_feature_slot: false,
            },
            protect,
        )
    }

    /// Replaces `style_layer` at `coords` with uniform feature metadata, reusing an already-uploaded
    /// shared vertex buffer when `geometry.shared_geometry_key()` matches a resident sibling of the
    /// same encoding family, or falling back to `fallback_vertices` otherwise.
    pub fn replace_layer_geometry_uniform_shared_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: UniformLayerUpload<'_, TM, FM, V, I>,
        fallback_vertices: Option<&[V]>,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_uniform_shared_inner(
            queue,
            coords,
            upload,
            SharedUniformTarget {
                fallback_vertices,
                needs_feature_slot: true,
            },
            protect,
        )
    }

    fn replace_uniform_shared_inner(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: UniformLayerUpload<'_, TM, FM, V, I>,
        shared: SharedUniformTarget<'_, V>,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let source = self.resolve_shared_vertex_source(
            coords,
            &upload.style_layer,
            upload.geometry,
            shared.fallback_vertices,
        )?;
        let plan =
            self.plan_uniform_slab_request(coords, &upload, &source, shared.needs_feature_slot)?;
        let reserved = self.reserve_shared_uniform_slabs(
            queue,
            coords,
            &upload.style_layer.type_,
            &source,
            &plan,
            protect,
        )?;
        let allocation_id = self.revision.wrapping_add(1);
        let style_layer_id = upload.style_layer.layer_id();
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            vertex_page: reserved.alloc.vertex_page,
            index_page: reserved.alloc.index_page,
            layer_page: reserved.alloc.layer_page,
            feature_page: reserved.alloc.feature_page,
            index_u16: reserved.alloc.index_u16,
            fm_kind: reserved.alloc.fm_kind,
            shared_vertex_family: plan.family,
            coords,
            style_layer_id,
            style_layer: std::sync::Arc::clone(&upload.style_layer),
            usable_indices: upload.geometry.usable_indices,
            first_vertex: reserved.first_vertex,
            first_index: reserved.first_index,
            buffer_vertices: reserved.alloc.vertices,
            buffer_indices: reserved.alloc.indices,
            buffer_layer_metadata: reserved.alloc.layer_metadata,
            buffer_feature_metadata: reserved.alloc.feature_metadata,
            uniform_feature_metadata: self.decoupled_feature_metadata,
            dual_feature_metadata: false,
            dynamic_circle_metadata: false,
            shared_geometry_key: source.shared_key,
            evaluated_zoom: None,
            u16_chunks: plan.u16_chunk_ranges.clone(),
            compact_extrusion: plan.use_compact_extrusion,
            opaque_color: false,
        };
        self.write_uniform_slot_buffers(
            queue,
            &entry,
            &upload,
            &source,
            &plan,
            reserved.uploaded_vertices,
        );
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

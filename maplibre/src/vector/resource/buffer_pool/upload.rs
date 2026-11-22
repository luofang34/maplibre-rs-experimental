use super::*;

mod slab_ops;
pub(super) use slab_ops::element_index;

/// Layer geometry and uniform per-feature metadata payload for pool replacement.
pub struct UniformLayerUpload<'a, TM, FM, V: Pod, I: Pod> {
    /// Style layer definition associated with this geometry.
    pub style_layer: std::sync::Arc<StyleLayer>,
    /// Tile-space geometry buffer to upload.
    pub geometry: &'a OverAlignedVertexBuffer<V, I>,
    /// Uniform per-layer shader metadata.
    pub layer_metadata: TM,
    /// Uniform per-feature metadata value repeated for feature records.
    pub uniform_feature: FM,
    /// Number of vertex/feature metadata records required.
    pub records: usize,
}

/// Layer geometry and per-feature metadata payload for pool allocation or replacement.
pub struct LayerGeometryUpload<'a, TM, FM, V: Pod, I: Pod> {
    /// Style layer definition associated with this geometry.
    pub style_layer: std::sync::Arc<StyleLayer>,
    /// Tile-space geometry buffer to upload.
    pub geometry: &'a OverAlignedVertexBuffer<V, I>,
    /// Uniform per-layer shader metadata.
    pub layer_metadata: TM,
    /// Per-feature metadata records matching the geometry.
    pub feature_metadata: &'a [FM],
}

struct PreparedGeometryPlan<'a> {
    strides: ElementStrides,
    vertices: u64,
    prepacked_verts: Option<(&'a [crate::render::shaders::GpuShaderVertex], bool)>,
    dual_fm: bool,
    req: SlabRequest,
    u16_chunk_ranges: Option<u16_chunks::U16Chunks>,
    layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Uploads a layer atomically, growing slabs or evicting older allocations when required.
    pub fn allocate_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: impl Into<std::sync::Arc<StyleLayer>>,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        self.allocate_layer_geometry_unless(
            queue,
            coords,
            LayerGeometryUpload {
                style_layer: style_layer.into(),
                geometry,
                layer_metadata,
                feature_metadata,
            },
            |_| false,
        )
    }

    /// Uploads a layer atomically, refusing to evict allocations of tiles matching `protect`.
    pub fn allocate_layer_geometry_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: LayerGeometryUpload<'_, TM, FM, V, I>,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let plan = self.prepare_geometry_plan(
            coords,
            &upload.style_layer,
            upload.geometry,
            upload.feature_metadata.len() as u64,
        )?;
        let alloc = self.reserve_allocation(
            Some(queue),
            plan.req,
            plan.strides,
            plan.layer_meta_key,
            None,
            &mut protect,
        )?;
        let (entry, allocation_id) = self.build_allocated_entry(coords, &upload, &plan, alloc)?;
        self.write_allocated_layer(queue, &entry, &upload, &plan);
        self.index.push_back(entry);
        self.revision = allocation_id;
        Ok(())
    }

    fn prepare_geometry_plan<'a>(
        &self,
        coords: WorldTileCoords,
        style_layer: &StyleLayer,
        geometry: &'a OverAlignedVertexBuffer<V, I>,
        records: u64,
    ) -> Result<PreparedGeometryPlan<'a>, AllocationError> {
        let prepacked_info = Self::prepacked_info(geometry);
        let prepacked_verts = prepacked_info.map(|(v, _, cmp)| (v, cmp));
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let vertices =
            prepacked_info.map_or(geometry.buffer.vertices.len(), |(_, len, _)| len) as u64;
        let dual_fm = self.decoupled_feature_metadata && vertices > 0 && records == 2 * vertices;
        if records > vertices && !dual_fm {
            return Err(AllocationError::FeatureMetadata { vertices, records });
        }
        let fm_kind = if strides.feature_metadata == 0
            || (self.decoupled_feature_metadata && records == 0)
        {
            FmStorageKind::None
        } else if !self.decoupled_feature_metadata || (!dual_fm && self.page_factory.is_some()) {
            FmStorageKind::Paired
        } else {
            FmStorageKind::Standalone
        };
        let allow_chunks = u16_chunks::layer_allows_u16_chunks(style_layer);
        let (index_u16, index_bytes, u16_chunk_ranges) =
            self.plan_geometry_index_upload(geometry, vertices, strides, allow_chunks);
        let req = SlabRequest {
            vertex_bytes: vertices * strides.vertex,
            vertex_elem_align: V::vertex_element_alignment(&style_layer.type_),
            index_bytes,
            index_u16,
            layer_bytes: strides.layer_metadata,
            feature_bytes: records * strides.feature_metadata,
            fm_kind,
        };
        self.validate_request(req, strides)?;
        let layer_meta_key = self.layer_meta_share_key(style_layer.layer_id(), coords);
        Ok(PreparedGeometryPlan {
            strides,
            vertices,
            prepacked_verts,
            dual_fm,
            req,
            u16_chunk_ranges,
            layer_meta_key,
        })
    }

    fn build_allocated_entry(
        &mut self,
        coords: WorldTileCoords,
        upload: &LayerGeometryUpload<'_, TM, FM, V, I>,
        plan: &PreparedGeometryPlan<'_>,
        alloc: SlabAllocation,
    ) -> Result<(IndexEntry, u64), AllocationError> {
        let idx_stride = plan.strides.active_index_stride(alloc.index_u16);
        let first_vertex = alloc
            .vertices
            .start
            .checked_div(plan.strides.vertex)
            .unwrap_or(0);
        let first_index = alloc.indices.start.checked_div(idx_stride).unwrap_or(0);
        let (first_vertex, first_index) =
            match (element_index(first_vertex), element_index(first_index)) {
                (Ok(fv), Ok(fi)) => (fv, fi),
                (Err(err), _) | (_, Err(err)) => {
                    self.abort_allocation(alloc, true, plan.layer_meta_key);
                    return Err(err);
                }
            };
        let allocation_id = self.revision.wrapping_add(1);
        let dynamic_circle_metadata = plan.dual_fm
            && matches!(
                upload.style_layer.paint.as_ref(),
                Some(crate::style::layer::LayerPaint::Circle(paint))
                    if paint.stroke_color_per_feature().is_none()
            );
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            vertex_page: alloc.vertex_page,
            index_page: alloc.index_page,
            layer_page: alloc.layer_page,
            feature_page: alloc.feature_page,
            index_u16: alloc.index_u16,
            fm_kind: alloc.fm_kind,
            shared_vertex_family: V::encoding_family(&upload.style_layer.type_),
            coords,
            style_layer_id: upload.style_layer.layer_id(),
            style_layer: std::sync::Arc::clone(&upload.style_layer),
            usable_indices: upload.geometry.usable_indices,
            first_vertex,
            first_index,
            buffer_vertices: alloc.vertices,
            buffer_indices: alloc.indices,
            buffer_layer_metadata: alloc.layer_metadata,
            buffer_feature_metadata: alloc.feature_metadata,
            uniform_feature_metadata: false,
            dual_feature_metadata: plan.dual_fm,
            dynamic_circle_metadata,
            shared_geometry_key: None,
            evaluated_zoom: None,
            u16_chunks: plan.u16_chunk_ranges.clone(),
            compact_extrusion: false,
            opaque_color: false,
        };
        Ok((entry, allocation_id))
    }

    fn write_allocated_layer(
        &mut self,
        queue: &Q,
        entry: &IndexEntry,
        upload: &LayerGeometryUpload<'_, TM, FM, V, I>,
        plan: &PreparedGeometryPlan<'_>,
    ) {
        if plan.req.vertex_bytes > 0 {
            self.write_geometry_vertices(
                queue,
                entry,
                plan.prepacked_verts,
                plan.vertices as usize,
                &upload.geometry.buffer.vertices,
            );
        }
        self.write_geometry_indices(queue, entry, upload.geometry, plan.req.index_bytes);
        if plan.req.layer_bytes > 0 && entry.layer_page != NO_SLAB {
            queue.write_buffer(
                self.metadata_for(entry),
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&upload.layer_metadata),
            );
        }
        if plan.req.feature_bytes > 0 {
            if entry.fm_kind == FmStorageKind::Paired {
                if let Some(v_slab) = self
                    .vertex_slabs
                    .get_mut(usize::from(entry.vertex_page))
                    .and_then(Option::as_mut)
                {
                    v_slab.paired_fm_dirty_end = v_slab
                        .paired_fm_dirty_end
                        .max(entry.buffer_feature_metadata.end);
                }
            }
            queue.write_buffer(
                self.feature_metadata_for(entry),
                entry.buffer_feature_metadata.start,
                bytemuck::cast_slice(upload.feature_metadata),
            );
        }
    }

    /// Replaces only this style layer with uniform feature metadata without heap scratch.
    pub fn replace_layer_geometry_uniform_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: UniformLayerUpload<'_, TM, FM, V, I>,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_layer_geometry_uniform_shared_unless(queue, coords, upload, None, protect)
    }

    /// Replaces only this style layer once the new geometry has been validated and uploaded.
    pub fn replace_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: impl Into<std::sync::Arc<StyleLayer>>,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        self.replace_layer_geometry_unless(
            queue,
            coords,
            LayerGeometryUpload {
                style_layer: style_layer.into(),
                geometry,
                layer_metadata,
                feature_metadata,
            },
            |_| false,
        )
    }

    /// Replaces only this style layer while refusing to evict tiles matching `protect`.
    pub fn replace_layer_geometry_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        upload: LayerGeometryUpload<'_, TM, FM, V, I>,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        let style_layer_id = upload.style_layer.layer_id();
        self.allocate_layer_geometry_unless(queue, coords, upload, protect)?;
        let removed = self
            .index
            .remove_layer(coords, style_layer_id, Some(self.revision));
        for entry in &removed {
            self.free_entry_ranges(entry);
        }
        if !removed.is_empty() {
            self.reclaim_empty_pages_with_queue(1, Some(queue));
        }
        Ok(())
    }
}

use super::*;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Returns `(bytes, copy_aligned_bytes)` for `elements` items of `stride` bytes.
    fn align(
        stride: wgpu::BufferAddress,
        elements: wgpu::BufferAddress,
        usable_elements: wgpu::BufferAddress,
    ) -> (wgpu::BufferAddress, wgpu::BufferAddress) {
        let bytes = elements * stride;
        let usable_bytes = usable_elements * stride;
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let padding = (align - usable_bytes % align) % align;
        (bytes, usable_bytes + padding)
    }

    /// Rewrites a layer's vertices in place when spatial elevations change without changing vertex
    /// count.
    pub fn update_vertices(&self, queue: &Q, entry: &IndexEntry, vertices: &[V]) {
        let mut scratch = Vec::new();
        let gpu_vertices = V::encode_vertices(&entry.style_layer.type_, vertices, &mut scratch);
        let bytes = bytemuck::cast_slice(gpu_vertices);
        let allocated = entry.buffer_vertices.end - entry.buffer_vertices.start;
        if bytes.len() as u64 != allocated
            || !(bytes.len() as u64).is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = bytes.len(),
                "vertex update skipped: size differs from the allocation"
            );
            return;
        }
        queue.write_buffer(self.vertices_for(entry), entry.buffer_vertices.start, bytes);
    }

    /// Rewrites a layer's per-layer paint metadata in place.
    #[tracing::instrument(skip_all)]
    pub fn update_layer_metadata(&self, queue: &Q, entry: &IndexEntry, layer_metadata: TM) {
        let layer_metadata_stride = size_of::<TM>() as wgpu::BufferAddress;
        if layer_metadata_stride == 0 || entry.layer_page == NO_SLAB {
            return;
        }
        let (layer_metadata_bytes, aligned_layer_metadata_bytes) =
            Self::align(layer_metadata_stride, 1, 1);

        let allocated = entry.buffer_layer_metadata.end - entry.buffer_layer_metadata.start;
        if allocated != layer_metadata_bytes {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = layer_metadata_bytes,
                "layer metadata update skipped: size differs from the allocation"
            );
            return;
        }

        queue.write_buffer(
            self.metadata_for(entry),
            entry.buffer_layer_metadata.start,
            &bytemuck::cast_slice(&[layer_metadata])[0..aligned_layer_metadata_bytes as usize],
        );
    }

    /// Rewrites a layer's per-vertex feature paint metadata in place.
    #[tracing::instrument(skip_all)]
    pub fn update_feature_metadata(&self, queue: &Q, entry: &IndexEntry, feature_metadata: &[FM]) {
        let feature_metadata_stride = size_of::<FM>() as wgpu::BufferAddress;
        let (feature_metadata_bytes, aligned_feature_metadata_bytes) = Self::align(
            feature_metadata_stride,
            feature_metadata.len() as wgpu::BufferAddress,
            feature_metadata.len() as wgpu::BufferAddress,
        );

        let allocated = entry.buffer_feature_metadata.end - entry.buffer_feature_metadata.start;
        if allocated != feature_metadata_bytes
            || feature_metadata_bytes != aligned_feature_metadata_bytes
        {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = feature_metadata_bytes,
                aligned = aligned_feature_metadata_bytes,
                "feature metadata update skipped: size differs from the allocation"
            );
            return;
        }
        if aligned_feature_metadata_bytes == 0 {
            return;
        }

        queue.write_buffer(
            self.feature_metadata_for(entry),
            entry.buffer_feature_metadata.start,
            &bytemuck::cast_slice(feature_metadata)[0..aligned_feature_metadata_bytes as usize],
        );
    }

    /// Rewrites a contiguous `[start_element..start_element + slice.len()]` sub-range of a
    /// layer's per-vertex feature metadata in place without re-uploading clean vertices.
    pub fn update_feature_metadata_range(
        &self,
        queue: &Q,
        entry: &IndexEntry,
        start_element: usize,
        slice: &[FM],
    ) {
        if slice.is_empty() {
            return;
        }
        let stride = size_of::<FM>() as wgpu::BufferAddress;
        let offset = (start_element as wgpu::BufferAddress) * stride;
        let (bytes, aligned_bytes) = Self::align(
            stride,
            slice.len() as wgpu::BufferAddress,
            slice.len() as wgpu::BufferAddress,
        );
        let allocated = entry.buffer_feature_metadata.end - entry.buffer_feature_metadata.start;
        if offset + aligned_bytes > allocated
            || bytes != aligned_bytes
            || !offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offset,
                offered = bytes,
                "feature metadata sub-range update skipped: out of bounds or unaligned"
            );
            return;
        }
        queue.write_buffer(
            self.feature_metadata_for(entry),
            entry.buffer_feature_metadata.start + offset,
            bytemuck::cast_slice(slice),
        );
    }

    /// Rewrites all feature metadata slots of `entry` with a single uniform `value` without heap scratch.
    pub fn fill_feature_metadata(&self, queue: &Q, entry: &IndexEntry, value: FM) {
        let stride = size_of::<FM>() as wgpu::BufferAddress;
        if stride == 0 {
            return;
        }
        let allocated = entry.buffer_feature_metadata.end - entry.buffer_feature_metadata.start;
        let count = (allocated / stride) as usize;
        if count == 0 {
            return;
        }
        Self::write_repeated_feature_metadata(
            queue,
            self.feature_metadata_for(entry),
            entry.buffer_feature_metadata.start,
            value,
            count,
        );
    }

    /// Rewrites a layer's indices in place, downcasting `u32 -> u16` when `entry` resides in a
    /// `u16` index slab.
    pub fn update_indices(&self, queue: &Q, entry: &IndexEntry, indices: &[I]) {
        if entry.index_page == NO_SLAB {
            return;
        }
        let allocated = entry.buffer_indices.end - entry.buffer_indices.start;
        if entry.index_u16 && size_of::<I>() == size_of::<u32>() {
            let u32_slice: &[u32] = bytemuck::cast_slice(indices);
            let usable = (entry.usable_indices as usize).min(u32_slice.len());
            let padded = usable.next_multiple_of(2);
            let bytes = (padded as u64) * 2;
            if bytes > allocated || bytes == 0 {
                return;
            }
            let mut u16_buf = Vec::with_capacity(padded);
            u16_buf.extend(u32_slice[..usable].iter().map(|&idx| idx as u16));
            if u16_buf.len() < padded {
                u16_buf.push(0);
            }
            queue.write_buffer(
                self.indices_for(entry),
                entry.buffer_indices.start,
                bytemuck::cast_slice(&u16_buf),
            );
            return;
        }

        let bytes = std::mem::size_of_val(indices) as wgpu::BufferAddress;
        if bytes > allocated || !bytes.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            tracing::error!(
                coords = %entry.coords,
                layer = %entry.style_layer.id,
                allocated,
                offered = bytes,
                "index update skipped: it does not fit the allocation"
            );
            return;
        }
        if bytes == 0 {
            return;
        }
        queue.write_buffer(
            self.indices_for(entry),
            entry.buffer_indices.start,
            bytemuck::cast_slice(indices),
        );
    }

    /// Updates the cached [`StyleLayer`] on every resident allocation of `style_layer.id` whose
    /// GPU-side definition changed, bumping [`IndexEntry::content_revision`] and returning the
    /// tile coordinates touched.
    pub fn sync_style_layer(&mut self, style_layer: &StyleLayer) -> Vec<WorldTileCoords> {
        let mut touched = Vec::new();
        let target_id = style_layer.layer_id();
        for tile_layers in self.index.iter_mut() {
            for entry in tile_layers {
                if entry.style_layer_id == target_id
                    && !entry.style_layer.same_gpu_definition(style_layer)
                {
                    entry.style_layer = style_layer.clone();
                    entry.style_layer_id = target_id;
                    entry.paint_revision = entry.paint_revision.wrapping_add(1);
                    touched.push(entry.coords);
                }
            }
        }
        if !touched.is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
        touched
    }

    /// Advances [`IndexEntry::content_revision`] for one resident `(coords, id)` entry after an
    /// in-place GPU metadata update so terrain drapes and symbol placement refresh.
    pub fn bump_layer_paint_revision(&mut self, coords: WorldTileCoords, id: &str) -> bool {
        let mut bumped = false;
        let target_id = crate::style::StyleLayerId::intern(id);
        if let Some(layers) = self.index.get_layers_mut(coords) {
            for entry in layers {
                if entry.style_layer_id == target_id {
                    entry.paint_revision = entry.paint_revision.wrapping_add(1);
                    bumped = true;
                }
            }
        }
        if bumped {
            self.revision = self.revision.wrapping_add(1);
        }
        bumped
    }
}

use super::*;

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Uploads a layer atomically, evicting older allocations when required.
    pub fn allocate_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let sizes = [
            geometry.buffer.vertices.len() as u64 * size_of::<V>() as u64,
            geometry.buffer.indices.len() as u64 * size_of::<I>() as u64,
            size_of::<TM>() as u64,
            feature_metadata.len() as u64 * size_of::<FM>() as u64,
        ];
        self.validate_sizes(sizes)?;
        let entry = IndexEntry {
            coords,
            style_layer,
            usable_indices: geometry.usable_indices,
            buffer_vertices: self.reserve(
                sizes[0],
                BackingBufferType::Vertices,
                self.vertices.inner_size,
            )?,
            buffer_indices: self.reserve(
                sizes[1],
                BackingBufferType::Indices,
                self.indices.inner_size,
            )?,
            buffer_layer_metadata: self.reserve(
                sizes[2],
                BackingBufferType::Metadata,
                self.layer_metadata.inner_size,
            )?,
            buffer_feature_metadata: self.reserve(
                sizes[3],
                BackingBufferType::FeatureMetadata,
                self.feature_metadata.inner_size,
            )?,
        };
        queue.write_buffer(
            &self.vertices.inner,
            entry.buffer_vertices.start,
            bytemuck::cast_slice(&geometry.buffer.vertices),
        );
        queue.write_buffer(
            &self.indices.inner,
            entry.buffer_indices.start,
            bytemuck::cast_slice(&geometry.buffer.indices),
        );
        queue.write_buffer(
            &self.layer_metadata.inner,
            entry.buffer_layer_metadata.start,
            bytemuck::bytes_of(&layer_metadata),
        );
        queue.write_buffer(
            &self.feature_metadata.inner,
            entry.buffer_feature_metadata.start,
            bytemuck::cast_slice(feature_metadata),
        );
        self.index.push_back(entry);
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn validate_sizes(&self, sizes: [u64; 4]) -> Result<(), AllocationError> {
        let buffers = [
            &self.vertices,
            &self.indices,
            &self.layer_metadata,
            &self.feature_metadata,
        ];
        for (buffer, bytes) in buffers.into_iter().zip(sizes) {
            if bytes > buffer.inner_size {
                return Err(AllocationError::Capacity {
                    buffer: buffer.typ,
                    requested: bytes,
                    capacity: buffer.inner_size,
                });
            }
            if bytes % wgpu::COPY_BUFFER_ALIGNMENT != 0 {
                return Err(AllocationError::Alignment {
                    buffer: buffer.typ,
                    bytes,
                });
            }
        }
        Ok(())
    }

    fn reserve(
        &mut self,
        requested: u64,
        buffer: BackingBufferType,
        capacity: u64,
    ) -> Result<Range<u64>, AllocationError> {
        self.index
            .make_room(requested, buffer, capacity)
            .ok_or(AllocationError::Capacity {
                buffer,
                requested,
                capacity,
            })
    }
}

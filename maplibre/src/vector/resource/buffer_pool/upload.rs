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
        let (vertex_stride, record_stride) = (size_of::<V>() as u64, size_of::<FM>() as u64);
        let (vertices, records) = (
            geometry.buffer.vertices.len() as u64,
            feature_metadata.len() as u64,
        );
        if records > vertices {
            return Err(AllocationError::FeatureMetadata { vertices, records });
        }
        // Vertices start on a whole element, so the layer draws from the whole buffer with
        // its first vertex as the base vertex; its feature records sit at the same index.
        let buffer_vertices = self.reserve(
            sizes[0],
            BackingBufferType::Vertices,
            (self.vertex_capacity(), vertex_stride),
        )?;
        let first_vertex = buffer_vertices.start / vertex_stride;
        let buffer_indices = self.reserve(
            sizes[1],
            BackingBufferType::Indices,
            (self.indices.inner_size, size_of::<I>() as u64),
        )?;
        let entry = IndexEntry {
            allocation_id: self.revision.wrapping_add(1),
            coords,
            style_layer,
            usable_indices: geometry.usable_indices,
            first_vertex: element_index(first_vertex)?,
            first_index: element_index(buffer_indices.start / size_of::<I>() as u64)?,
            buffer_vertices,
            buffer_indices,
            buffer_layer_metadata: self.reserve(
                sizes[2],
                BackingBufferType::Metadata,
                (self.layer_metadata.inner_size, size_of::<TM>() as u64),
            )?,
            buffer_feature_metadata: first_vertex * record_stride
                ..first_vertex * record_stride + sizes[3],
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

    /// Replaces only this style layer once the new geometry has been validated and uploaded.
    pub fn replace_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        let id = style_layer.id.clone();
        self.allocate_layer_geometry(
            queue,
            coords,
            style_layer,
            geometry,
            layer_metadata,
            feature_metadata,
        )?;
        self.index.remove_layer(coords, &id, Some(self.revision));
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

    /// Bytes of the vertex buffer the pool allocates from: as many whole vertices as both the
    /// vertex buffer and the feature metadata buffer, which holds a record per vertex, hold.
    fn vertex_capacity(&self) -> u64 {
        let elements = (self.vertices.inner_size / size_of::<V>() as u64)
            .min(self.feature_metadata.inner_size / size_of::<FM>() as u64);
        elements * size_of::<V>() as u64
    }

    fn reserve(
        &mut self,
        requested: u64,
        buffer: BackingBufferType,
        (capacity, align): (u64, u64),
    ) -> Result<Range<u64>, AllocationError> {
        self.index
            .make_room(requested, buffer, (capacity, align))
            .ok_or(AllocationError::Capacity {
                buffer,
                requested,
                capacity,
            })
    }
}

/// An element index as the 32 bits a draw call takes; a pool beyond them is not drawable.
fn element_index(index: u64) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}

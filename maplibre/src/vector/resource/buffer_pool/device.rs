use super::*;
impl<V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<wgpu::Queue, wgpu::Buffer, V, I, TM, FM> {
    pub fn from_device(device: &wgpu::Device) -> Self {
        Self::from_device_with_sizes(device, BufferPoolSizes::default())
    }

    /// A pool whose buffers hold `sizes` elements, each clipped to the device's largest buffer.
    /// Too small a pool has tiles unloaded and reloaded from frame to frame.
    pub fn from_device_with_sizes(device: &wgpu::Device, sizes: BufferPoolSizes) -> Self {
        let largest = device.limits().max_buffer_size;
        let fitting = |element: usize, count: wgpu::BufferAddress| {
            fitting_buffer_size(element, count, largest)
        };
        let vertex_buffer_desc = wgpu::BufferDescriptor {
            label: Some("vertex buffer"),
            size: fitting(size_of::<V>(), sizes.vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        };

        let indices_buffer_desc = wgpu::BufferDescriptor {
            label: Some("indices buffer"),
            size: fitting(size_of::<I>(), sizes.indices),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        };

        let feature_metadata_desc = wgpu::BufferDescriptor {
            label: Some("feature metadata buffer"),
            size: fitting(size_of::<FM>(), sizes.feature_metadata),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        };

        let layer_metadata_desc = wgpu::BufferDescriptor {
            label: Some("layer metadata buffer"),
            size: fitting(size_of::<TM>(), sizes.layer_metadata),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        };

        BufferPool::new(
            BackingBufferDescriptor::new(
                device.create_buffer(&vertex_buffer_desc),
                vertex_buffer_desc.size,
            ),
            BackingBufferDescriptor::new(
                device.create_buffer(&indices_buffer_desc),
                indices_buffer_desc.size,
            ),
            BackingBufferDescriptor::new(
                device.create_buffer(&layer_metadata_desc),
                layer_metadata_desc.size,
            ),
            BackingBufferDescriptor::new(
                device.create_buffer(&feature_metadata_desc),
                feature_metadata_desc.size,
            ),
        )
    }
}

impl<V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<wgpu::Queue, wgpu::Buffer, V, I, TM, FM> {
    /// Binds a layer's indices, vertices and feature metadata (at `feature_slot`) and returns
    /// the first index and base vertex to draw its indices with. With `whole` the pool's
    /// buffers are bound whole, so consecutive layers keep them bound; without, as on WebGL2,
    /// which has no base vertex, the layer's own ranges are bound and drawn from zero.
    pub fn bind_layer<'w>(
        &self,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        entry: &IndexEntry,
        (feature_slot, whole): (u32, bool),
    ) -> (u32, i32) {
        let format = crate::render::INDEX_FORMAT;
        if whole {
            pass.set_index_buffer(self.indices.inner.slice(..), format);
            pass.set_vertex_buffer(0, self.vertices.inner.slice(..));
            pass.set_vertex_buffer(feature_slot, self.feature_metadata.inner.slice(..));
            (entry.whole_buffer_indices().start, entry.base_vertex())
        } else {
            pass.set_index_buffer(
                self.indices.inner.slice(entry.indices_buffer_range()),
                format,
            );
            pass.set_vertex_buffer(0, self.vertices.inner.slice(entry.vertices_buffer_range()));
            pass.set_vertex_buffer(
                feature_slot,
                self.feature_metadata
                    .inner
                    .slice(entry.feature_metadata_buffer_range()),
            );
            (0, 0)
        }
    }
}

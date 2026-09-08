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

use super::*;

impl<V: Pod, I: Pod, TM: Pod, FM: Pod>
    BufferPool<crate::render::upload_queue::UploadQueue, wgpu::Buffer, V, I, TM, FM>
{
    /// Creates an on-demand paged GPU pool with [`BufferPoolSizes::default`] maximum capacity.
    pub fn from_device(device: &wgpu::Device) -> Self {
        Self::from_device_with_sizes(device, BufferPoolSizes::default())
    }

    /// Creates an on-demand paged GPU pool whose total element capacities across pages are bounded
    /// by `sizes`, with each page clipped to the device's `max_buffer_size`.
    ///
    /// Zero GPU buffers are allocated until the first non-empty geometry upload arrives, avoiding
    /// upfront hundreds-of-megabytes allocations and keeping [`BufferPoolSizes::EMPTY`] at `0` B.
    pub fn from_device_with_sizes(device: &wgpu::Device, sizes: BufferPoolSizes) -> Self {
        let max_page_bytes = device.limits().max_buffer_size;
        let device = device.clone();
        let mut pool = Self::with_page_factory(sizes, max_page_bytes, move |typ, logical_bytes| {
            let physical = logical_bytes.max(16).next_multiple_of(16);
            let (label, usage) = match typ {
                BackingBufferType::Vertices => (
                    "vertex buffer page",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::Indices => (
                    "indices buffer page",
                    wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::Metadata => (
                    "layer metadata buffer page",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
                BackingBufferType::FeatureMetadata => (
                    "feature metadata buffer page",
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
            };
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: physical,
                usage,
                mapped_at_creation: false,
            })
        });
        pool.page_destroyer = Some(page::PageDestroyer(|page| {
            page.vertices.inner.destroy();
            page.indices.inner.destroy();
            page.layer_metadata.inner.destroy();
            page.feature_metadata.inner.destroy();
        }));
        pool
    }

    /// Total GPU buffer bytes currently physically allocated on the device across all live pages.
    pub fn device_buffer_bytes(&self) -> wgpu::BufferAddress {
        self.pages
            .iter()
            .flatten()
            .map(|page| {
                page.vertices.inner.size()
                    + page.indices.inner.size()
                    + page.layer_metadata.inner.size()
                    + page.feature_metadata.inner.size()
            })
            .sum()
    }

    /// Binds a layer's indices, vertices and feature metadata (at `feature_slot`) from its page
    /// and returns the first index and base vertex to draw its indices with. With `whole` the
    /// page's buffers are bound whole, so consecutive layers on the same page keep them bound;
    /// without, as on WebGL2, which has no base vertex, the layer's own ranges are bound and
    /// drawn from zero.
    pub fn bind_layer<'w>(
        &self,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        entry: &IndexEntry,
        (feature_slot, whole): (u32, bool),
    ) -> (u32, i32) {
        let format = crate::render::INDEX_FORMAT;
        let page = self.page_of(entry);
        if whole {
            pass.set_index_buffer(page.indices.inner.slice(..), format);
            pass.set_vertex_buffer(0, page.vertices.inner.slice(..));
            pass.set_vertex_buffer(feature_slot, page.feature_metadata.inner.slice(..));
            (entry.whole_buffer_indices().start, entry.base_vertex())
        } else {
            pass.set_index_buffer(
                page.indices.inner.slice(entry.indices_buffer_range()),
                format,
            );
            pass.set_vertex_buffer(0, page.vertices.inner.slice(entry.vertices_buffer_range()));
            pass.set_vertex_buffer(
                feature_slot,
                page.feature_metadata
                    .inner
                    .slice(entry.feature_metadata_buffer_range()),
            );
            (0, 0)
        }
    }
}

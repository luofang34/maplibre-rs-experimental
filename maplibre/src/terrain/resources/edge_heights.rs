//! Per-tile stitched edge height texture for WebGL2-compatible vertex-stage lookup.

use super::{TerrainResources, UNIFORM_CAPACITY};

/// One row of the edge height texture: a tile's [`EdgeHeights`](crate::terrain::queue_system::edges::EdgeHeights).
pub(super) const EDGE_BYTES_PER_TILE: u32 =
    std::mem::size_of::<crate::terrain::queue_system::edges::EdgeHeights>() as u32;
/// Edge height texels per tile, four edges to the texel.
pub(super) const EDGE_TEXELS_PER_TILE: u32 =
    EDGE_BYTES_PER_TILE / std::mem::size_of::<[f32; 4]>() as u32;

/// Clamps the per-frame edge texture row capacity to the device's 2D texture dimension limit.
pub(super) fn edge_row_capacity(max_texture_dimension_2d: u32) -> u32 {
    (UNIFORM_CAPACITY as u32)
        .min(max_texture_dimension_2d)
        .max(1)
}

/// The edge height texture's extent for `tiles` tiles, one row each.
pub(super) fn edge_texture_size(tiles: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: EDGE_TEXELS_PER_TILE,
        height: tiles,
        depth_or_array_layers: 1,
    }
}

pub(crate) struct EdgeHeightTexture {
    texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    capacity: usize,
}

impl EdgeHeightTexture {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let rows = edge_row_capacity(device.limits().max_texture_dimension_2d);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain edge heights"),
            size: edge_texture_size(rows),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            capacity: rows as usize,
        }
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Drop for EdgeHeightTexture {
    fn drop(&mut self) {
        self.texture.destroy();
    }
}

impl TerrainResources {
    /// Writes the per-tile edge height blocks.
    pub(crate) fn write_edges(
        &self,
        queue: &crate::render::upload_queue::UploadQueue,
        edges: &[crate::terrain::queue_system::edges::EdgeHeights],
    ) {
        let count = edges.len().min(self.edge_heights.capacity());
        if count == 0 {
            return;
        }
        queue.write_texture(
            self.edge_heights.texture.as_image_copy(),
            bytemuck::cast_slice(&edges[..count]),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(EDGE_BYTES_PER_TILE),
                rows_per_image: None,
            },
            edge_texture_size(count as u32),
        );
    }
}

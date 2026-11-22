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
    uploaded_generation: Option<u64>,
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
            uploaded_generation: None,
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
    /// Maximum tile rows supported by the edge height texture.
    pub(crate) fn edge_capacity(&self) -> usize {
        self.edge_heights.capacity()
    }

    /// Writes the edge height blocks only when `generation == 0` (fallback) or `generation` differs
    /// from the last uploaded generation, uploading only changed contiguous row runs when resident.
    pub(crate) fn write_edges_if_stale(
        &mut self,
        queue: &crate::render::upload_queue::UploadQueue,
        generation: u64,
        edges: &[crate::terrain::queue_system::edges::EdgeHeights],
        dirty_rows: &[u32],
    ) {
        if generation != 0 && self.edge_heights.uploaded_generation == Some(generation) {
            return;
        }
        let count = edges.len().min(self.edge_heights.capacity());
        if count == 0 {
            return;
        }
        if generation != 0
            && self.edge_heights.uploaded_generation.is_some()
            && !dirty_rows.is_empty()
            && dirty_rows.len() * 2 < count
        {
            self.write_dirty_edge_runs(queue, &edges[..count], dirty_rows);
            self.edge_heights.uploaded_generation = Some(generation);
            return;
        }
        if generation == 0
            || self.edge_heights.uploaded_generation.is_none()
            || dirty_rows.is_empty()
            || dirty_rows.len() * 2 >= count
        {
            self.write_edges(queue, edges);
        }
        self.edge_heights.uploaded_generation = (generation != 0).then_some(generation);
    }

    fn write_dirty_edge_runs(
        &self,
        queue: &crate::render::upload_queue::UploadQueue,
        edges: &[crate::terrain::queue_system::edges::EdgeHeights],
        dirty_rows: &[u32],
    ) {
        let mut rows: Vec<usize> = dirty_rows
            .iter()
            .copied()
            .map(|r| r as usize)
            .filter(|&r| r < edges.len())
            .collect();
        if rows.is_empty() {
            return;
        }
        rows.sort_unstable();
        rows.dedup();
        let mut idx = 0;
        while idx < rows.len() {
            let start = rows[idx];
            let mut end = start + 1;
            while idx + 1 < rows.len() && rows[idx + 1] == end {
                idx += 1;
                end += 1;
            }
            if let Some(slice) = edges.get(start..end) {
                let copy = wgpu::TexelCopyTextureInfo {
                    texture: &self.edge_heights.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: start as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                };
                queue.write_texture(
                    copy,
                    bytemuck::cast_slice(slice),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(EDGE_BYTES_PER_TILE),
                        rows_per_image: None,
                    },
                    edge_texture_size((end - start) as u32),
                );
            }
            idx += 1;
        }
    }

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
        let Some(slice) = edges.get(..count) else {
            return;
        };
        queue.write_texture(
            self.edge_heights.texture.as_image_copy(),
            bytemuck::cast_slice(slice),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(EDGE_BYTES_PER_TILE),
                rows_per_image: None,
            },
            edge_texture_size(count as u32),
        );
    }
}

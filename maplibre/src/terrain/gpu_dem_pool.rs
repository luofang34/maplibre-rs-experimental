//! Shared GPU `(dim + 4) x (dim + 4)` elevation texture pool across terrain, hillshade, and colour relief.

use std::collections::HashMap;

use crate::{
    coords::WorldTileCoords,
    raster::RasterSourceId,
    render::{resource::Texture, settings::Msaa, upload_queue::UploadQueue},
    terrain::{DemRevision, DemTile, LoadedDem},
};

/// One resident GPU elevation texture in [`GpuDemPool`].
pub struct GpuDemEntry {
    /// `(dim + 4) x (dim + 4)` single-mip `Rgba8Unorm` texture holding 2-texel bordered DEM pixels.
    pub texture: Texture,
    /// Wrapping sample revision matching the uploaded [`LoadedDem::revision`].
    pub revision: u32,
    pub(crate) source_key: DemRevision,
}

/// Canonical GPU elevation texture registry shared by [`TerrainResources`](super::resources::TerrainResources)
/// and [`RasterResources`](crate::raster::resource::RasterResources).
///
/// A `raster-dem` source shared by 3D terrain, `hillshade`, and `color-relief` allocates and
/// uploads each `(source, coords)` DEM texture once on the GPU, updating it in place via
/// [`UploadQueue::write_texture`] when neighbour borders backfill.
#[derive(Default)]
pub struct GpuDemPool {
    textures: HashMap<(RasterSourceId, WorldTileCoords), GpuDemEntry>,
    uploads: u64,
}

impl GpuDemPool {
    /// Whether `(source, coords)` is missing from the GPU or holds a different [`LoadedDem`] revision.
    pub fn needs_upload(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        dem: &LoadedDem,
    ) -> bool {
        let expected = dem.revision_key();
        !self
            .textures
            .get(&(source.clone(), coords))
            .is_some_and(|entry| entry.source_key == expected)
    }

    /// Borrows the resident GPU entry for `(source, coords)`, if uploaded.
    pub fn get(&self, source: &RasterSourceId, coords: WorldTileCoords) -> Option<&GpuDemEntry> {
        self.textures.get(&(source.clone(), coords))
    }

    /// Uploads `dem` into the shared GPU pool, reusing an existing texture of matching stride in place.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        dem: &LoadedDem,
    ) -> &GpuDemEntry {
        let key = (source.clone(), coords);
        let source_key = dem.revision_key();
        let stride = dem.tile.stride();
        let reusable = self
            .textures
            .remove(&key)
            .map(|entry| entry.texture)
            .filter(|texture| texture.size.width == stride);
        let texture = reusable.unwrap_or_else(|| allocate_dem_texture(device, stride));
        write_dem_pixels(queue, &texture, &dem.tile);
        self.uploads = self.uploads.wrapping_add(1);
        self.textures.entry(key).or_insert(GpuDemEntry {
            texture,
            revision: dem.revision,
            source_key,
        })
    }

    /// Drops every source's GPU DEM texture at `coords`.
    pub fn drop_coords(&mut self, coords: WorldTileCoords) {
        self.textures.retain(|(_, at), _| *at != coords);
    }

    /// Drops one source's GPU DEM texture at `coords`.
    pub fn drop_source_coords(&mut self, source: &RasterSourceId, coords: WorldTileCoords) {
        self.textures.remove(&(source.clone(), coords));
    }

    /// Drops every GPU DEM texture belonging to `source`.
    pub fn drop_source(&mut self, source: &RasterSourceId) {
        self.textures.retain(|(id, _), _| id != source);
    }

    /// Number of unique `(source, coords)` DEM textures currently resident in GPU memory.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Total GPU texture memory in bytes held by resident DEM tiles in this pool.
    pub fn texture_bytes(&self) -> usize {
        self.textures
            .values()
            .map(|entry| (entry.texture.size.width * entry.texture.size.height * 4) as usize)
            .sum()
    }

    /// Cumulative `write_texture` uploads performed through this pool.
    pub fn upload_count(&self) -> u64 {
        self.uploads
    }
}

pub(crate) fn allocate_dem_texture(device: &wgpu::Device, stride: u32) -> Texture {
    Texture::new(
        Some("DEM tile"),
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        stride,
        stride,
        Msaa { samples: 1 },
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    )
}

pub(crate) fn write_dem_pixels(queue: &UploadQueue, texture: &Texture, dem: &DemTile) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        dem.pixels(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * dem.stride()),
            rows_per_image: Some(dem.stride()),
        },
        texture.size,
    );
}

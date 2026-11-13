//! Shared GPU `(dim + 4) x (dim + 4)` elevation texture pool across terrain, hillshade, and colour relief.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    coords::WorldTileCoords,
    raster::RasterSourceId,
    render::{resource::Texture, settings::Msaa, upload_queue::UploadQueue},
    terrain::{DemRevision, DemTile, LoadedDem},
};

/// Default hard LRU cap on GPU DEM textures held in [`GpuDemPool`].
pub const MAX_GPU_DEM_TEXTURES: usize = 256;

/// One resident GPU elevation texture in [`GpuDemPool`].
pub struct GpuDemEntry {
    /// `(dim + 4) x (dim + 4)` single-mip `Rgba8Unorm` texture holding 2-texel bordered DEM pixels.
    pub texture: Texture,
    /// Wrapping sample revision matching the uploaded [`LoadedDem::revision`].
    pub revision: u32,
    pub(crate) source_key: DemRevision,
    last_used: AtomicU64,
}

/// Canonical GPU elevation texture registry shared by [`TerrainResources`](super::resources::TerrainResources)
/// and [`RasterResources`](crate::raster::resource::RasterResources).
///
/// A `raster-dem` source shared by 3D terrain, `hillshade`, and `color-relief` allocates and
/// uploads each `(source, coords)` DEM texture once on the GPU, updating it in place via
/// [`UploadQueue::write_texture`] when neighbour borders backfill.
pub struct GpuDemPool {
    textures: HashMap<(RasterSourceId, WorldTileCoords), GpuDemEntry>,
    uploads: u64,
    tick: AtomicU64,
    max_textures: usize,
}

impl Default for GpuDemPool {
    fn default() -> Self {
        Self::with_capacity(MAX_GPU_DEM_TEXTURES)
    }
}

impl GpuDemPool {
    /// Creates an empty GPU DEM pool capped at `max_textures` least-recently-used entries.
    pub fn with_capacity(max_textures: usize) -> Self {
        Self {
            textures: HashMap::new(),
            uploads: 0,
            tick: AtomicU64::new(0),
            max_textures: max_textures.max(1),
        }
    }

    fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

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
        let entry = self.textures.get(&(source.clone(), coords))?;
        entry.last_used.store(self.next_tick(), Ordering::Relaxed);
        Some(entry)
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
        while self.textures.len() >= self.max_textures {
            let Some(oldest_key) = self
                .textures
                .iter()
                .min_by_key(|(_, entry)| entry.last_used.load(Ordering::Relaxed))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.textures.remove(&oldest_key);
        }
        let texture = reusable.unwrap_or_else(|| allocate_dem_texture(device, stride));
        write_dem_pixels(queue, &texture, &dem.tile);
        self.uploads = self.uploads.wrapping_add(1);
        let used = self.next_tick();
        self.textures.entry(key).or_insert(GpuDemEntry {
            texture,
            revision: dem.revision,
            source_key,
            last_used: AtomicU64::new(used),
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

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
mod tests {
    #![allow(clippy::expect_used)]

    use super::GpuDemPool;
    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        headless::create_headless_renderer,
        raster::RasterSourceId,
        terrain::{DemTile, LoadedDem},
    };

    #[test]
    fn gpu_dem_pool_evicts_least_recently_used_beyond_capacity() {
        futures::executor::block_on(async {
            let (_, renderer) = create_headless_renderer(32, 32, None)
                .await
                .expect("renderer");
            let mut pool = GpuDemPool::with_capacity(2);
            let source = RasterSourceId::from("dem");
            let img = image::RgbaImage::new(4, 4);
            let dem = LoadedDem::new(
                DemTile::from_image(&img, [256.0, 1.0, 1.0 / 256.0, -32768.0]).expect("dem"),
            );
            let [t0, t1, t2] = [0, 1, 2].map(|x| WorldTileCoords::from((x, 0, ZoomLevel::new(4))));

            pool.upload(&renderer.device, &renderer.queue, &source, t0, &dem);
            pool.upload(&renderer.device, &renderer.queue, &source, t1, &dem);
            // Touch t0 so t1 becomes the least recently used entry.
            assert!(pool.get(&source, t0).is_some());
            pool.upload(&renderer.device, &renderer.queue, &source, t2, &dem);

            assert_eq!(pool.texture_count(), 2);
            assert!(
                pool.get(&source, t0).is_some(),
                "recently touched DEM stays"
            );
            assert!(pool.get(&source, t2).is_some(), "newly uploaded DEM stays");
            assert!(pool.get(&source, t1).is_none(), "oldest DEM was evicted");
        })
    }
}

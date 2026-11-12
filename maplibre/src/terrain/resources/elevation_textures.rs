//! Uploads elevation samples while tracking replacements and border changes.

use std::sync::atomic::{AtomicU64, Ordering};

use super::TerrainResources;
use crate::{
    coords::WorldTileCoords,
    render::{resource::Texture, settings::Msaa},
    terrain::{DemRevision, DemTile, LoadedDem},
};

/// Hard LRU cap on terrain elevation textures bound simultaneously in [`TerrainResources`].
const MAX_TERRAIN_DEM_TEXTURES: usize = crate::terrain::gpu_dem_pool::MAX_GPU_DEM_TEXTURES;

static DEM_TOUCH_TICK: AtomicU64 = AtomicU64::new(0);

fn next_dem_tick() -> u64 {
    DEM_TOUCH_TICK
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
}

pub(super) struct DemTexture {
    pub(super) texture: Texture,
    revision: u32,
    source: Option<DemRevision>,
    last_used: AtomicU64,
}

impl TerrainResources {
    fn evict_excess_dem_textures(&mut self, keep: WorldTileCoords) {
        while self.dem_textures.len() >= MAX_TERRAIN_DEM_TEXTURES {
            let Some(oldest) = self
                .dem_textures
                .iter()
                .filter(|(coords, _)| **coords != keep)
                .min_by_key(|(_, entry)| entry.last_used.load(Ordering::Relaxed))
                .map(|(coords, _)| *coords)
            else {
                break;
            };
            self.dem_textures.remove(&oldest);
        }
    }

    pub(crate) fn bind_shared_dem(
        &mut self,
        coords: WorldTileCoords,
        texture: Texture,
        revision: u32,
        source: DemRevision,
    ) {
        if !self.dem_textures.contains_key(&coords) {
            self.evict_excess_dem_textures(coords);
        }
        self.dem_textures.insert(
            coords,
            DemTexture {
                texture,
                revision,
                source: Some(source),
                last_used: AtomicU64::new(next_dem_tick()),
            },
        );
    }

    pub(crate) fn upload_loaded_dem(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        coords: WorldTileCoords,
        dem: &LoadedDem,
    ) {
        if !self.needs_dem_upload(coords, dem) {
            return;
        }
        let source = dem.revision_key();
        self.upload_dem(device, queue, coords, &dem.tile, dem.revision);
        if let Some(entry) = self.dem_textures.get_mut(&coords) {
            entry.source = Some(source);
        }
    }

    /// Whether the uploaded copy of the DEM tile at `coords`, if any, is of another revision.
    pub(crate) fn needs_dem_upload(&self, coords: WorldTileCoords, dem: &LoadedDem) -> bool {
        let source = dem.revision_key();
        !self
            .dem_textures
            .get(&coords)
            .is_some_and(|entry| entry.source.as_ref() == Some(&source))
    }

    /// Whether a DEM tile has been uploaded.
    pub fn has_dem_texture(&self, coords: WorldTileCoords) -> bool {
        self.dem_textures.contains_key(&coords)
    }

    /// Revision of the uploaded copy of a DEM tile, if any.
    pub fn dem_revision(&self, coords: WorldTileCoords) -> Option<u32> {
        self.dem_textures.get(&coords).map(|dem| dem.revision)
    }

    /// Uploads the bordered pixels of a decoded DEM tile, reusing its texture across revisions.
    pub fn upload_dem(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        coords: WorldTileCoords,
        dem: &DemTile,
        revision: u32,
    ) {
        let reusable = self
            .dem_textures
            .remove(&coords)
            .map(|dem| dem.texture)
            .filter(|texture| texture.size.width == dem.stride());
        self.evict_excess_dem_textures(coords);
        let texture = reusable.unwrap_or_else(|| {
            Texture::new(
                Some("DEM tile"),
                device,
                wgpu::TextureFormat::Rgba8Unorm,
                dem.stride(),
                dem.stride(),
                Msaa { samples: 1 },
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            )
        });
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
        self.dem_textures.insert(
            coords,
            DemTexture {
                texture,
                revision,
                source: None,
                last_used: AtomicU64::new(next_dem_tick()),
            },
        );
    }

    /// Releases the DEM texture of a tile that left the store.
    pub fn drop_dem(&mut self, coords: WorldTileCoords) {
        self.dem_textures.remove(&coords);
    }

    /// DEM texture of a tile, or the flat stand-in while it loads.
    pub fn dem_texture(&self, coords: Option<WorldTileCoords>) -> &Texture {
        coords
            .and_then(|coords| self.dem_textures.get(&coords))
            .map_or(&self.empty_dem, |dem| {
                dem.last_used.store(next_dem_tick(), Ordering::Relaxed);
                &dem.texture
            })
    }

    /// DEM textures resident on the GPU.
    pub fn dem_texture_count(&self) -> usize {
        self.dem_textures.len()
    }
}

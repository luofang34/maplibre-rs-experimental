//! Uploads elevation samples while tracking replacements and border changes.

use super::TerrainResources;
use crate::{
    coords::WorldTileCoords,
    render::{resource::Texture, settings::Msaa},
    terrain::{DemRevision, DemTile, LoadedDem},
};

pub(super) struct DemTexture {
    pub(super) texture: Texture,
    revision: u32,
    source: Option<DemRevision>,
}

impl TerrainResources {
    pub(crate) fn upload_loaded_dem(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        coords: WorldTileCoords,
        dem: &LoadedDem,
    ) {
        let source = dem.revision_key();
        if self
            .dem_textures
            .get(&coords)
            .is_some_and(|entry| entry.source.as_ref() == Some(&source))
        {
            return;
        }
        self.upload_dem(device, queue, coords, &dem.tile, dem.revision);
        if let Some(entry) = self.dem_textures.get_mut(&coords) {
            entry.source = Some(source);
        }
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
        queue: &wgpu::Queue,
        coords: WorldTileCoords,
        dem: &DemTile,
        revision: u32,
    ) {
        let reusable = self
            .dem_textures
            .remove(&coords)
            .map(|dem| dem.texture)
            .filter(|texture| texture.size.width == dem.stride());
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
            .map_or(&self.empty_dem, |dem| &dem.texture)
    }

    /// DEM textures resident on the GPU.
    pub fn dem_texture_count(&self) -> usize {
        self.dem_textures.len()
    }
}

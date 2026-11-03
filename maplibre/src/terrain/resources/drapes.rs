//! Allocation and deferred content of terrain drape textures.
use std::collections::HashSet;

use super::TerrainResources;
use crate::{coords::WorldTileCoords, render::resource::Texture, terrain::drape_cache::DrapeState};

/// A drape texture with the view of its first level, which the drape pass draws into every
/// time the drape is redrawn; the texture's own view spans every level, which an attachment
/// may not.
pub(crate) struct DrapeTexture {
    texture: Texture,
    top_level: wgpu::TextureView,
}

impl DrapeTexture {
    fn new(texture: Texture) -> Self {
        let top_level = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("drape top level"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        Self { texture, top_level }
    }
}

impl TerrainResources {
    /// Gives a view tile a drape texture and reports what it holds.
    pub fn acquire_drape(
        &mut self,
        device: &wgpu::Device,
        coords: WorldTileCoords,
        fingerprint: u64,
        may_create: bool,
    ) -> DrapeState {
        let format = self.color_format;
        let size = self.drape_size;
        self.drapes.acquire(coords, fingerprint, may_create, || {
            DrapeTexture::new(Texture::new_mipmapped(
                Some("drape texture"),
                device,
                format,
                size,
                size,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ))
        })
    }

    /// Leaves a drape acquired this frame undrawn, to be drawn on the next.
    pub fn defer_drape(&mut self, coords: WorldTileCoords, state: DrapeState) {
        self.drapes.defer(coords, state);
    }

    /// Drape textures held, parked and spare together.
    pub fn drape_texture_total(&self) -> usize {
        self.drapes.total_textures()
    }

    /// Drops the parked and spare drape textures.
    pub fn shed_spare_drapes(&mut self) {
        self.drapes.shed_spares();
    }

    /// Fills the mip levels of a drape texture after its layers were drawn into it.
    pub fn generate_drape_mipmaps(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &Texture,
    ) {
        if let Some(mipmaps) = &self.mipmaps {
            mipmaps.generate(device, encoder, &texture.texture);
        }
    }

    /// Keeps the requested tile drapes active and parks other textures with their content.
    /// Older parked textures are recycled or dropped according to the cache's spare limits.
    pub fn retain_drapes(&mut self, keep: &HashSet<WorldTileCoords>) {
        self.drapes.retain(keep);
    }

    /// Drape texture of a view tile.
    pub fn drape_texture(&self, coords: WorldTileCoords) -> Option<&Texture> {
        self.drapes.get(coords).map(|drape| &drape.texture)
    }

    /// Drape texture of a view tile with the view of its first level, the one drawn into.
    pub(crate) fn drape_target(
        &self,
        coords: WorldTileCoords,
    ) -> Option<(&Texture, &wgpu::TextureView)> {
        self.drapes
            .get(coords)
            .map(|drape| (&drape.texture, &drape.top_level))
    }

    /// Drape textures held for tiles, and parked or waiting in the free list.
    pub fn drape_counts(&self) -> (usize, usize) {
        (
            self.drapes.len(),
            self.drapes.parked_len() + self.drapes.free_len(),
        )
    }
}

//! Allocation and deferred content of terrain drape textures.
use super::{TerrainResources, DRAPE_SIZE};
use crate::{coords::WorldTileCoords, render::resource::Texture, terrain::drape_cache::DrapeState};
use std::collections::HashSet;
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
        self.drapes.acquire(coords, fingerprint, may_create, || {
            Texture::new_mipmapped(
                Some("drape texture"),
                device,
                format,
                DRAPE_SIZE,
                DRAPE_SIZE,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            )
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
        self.mipmaps.generate(device, encoder, &texture.texture);
    }

    pub fn retain_drapes(&mut self, keep: &HashSet<WorldTileCoords>) {
        self.drapes.retain(keep);
    }

    /// Drape texture of a view tile.
    pub fn drape_texture(&self, coords: WorldTileCoords) -> Option<&Texture> {
        self.drapes.get(coords)
    }

    /// Drape textures held for tiles, and parked or waiting in the free list.
    pub fn drape_counts(&self) -> (usize, usize) {
        (
            self.drapes.len(),
            self.drapes.parked_len() + self.drapes.free_len(),
        )
    }
}

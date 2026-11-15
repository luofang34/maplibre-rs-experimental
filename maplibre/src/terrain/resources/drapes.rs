//! Allocation and deferred content of terrain drape textures.
//!
//! A drape belongs to a terrain tile of one view. Each view selects the source tiles of a
//! drape from its own covering, so two views draping the same terrain tile want different
//! content in it; with one texture between them each would find the other's content changed
//! and draw it again every frame, and what a view shows would depend on which other views
//! share its frame. The lookups by tile answer for the view chosen with
//! [`TerrainResources::show_drapes_of`], and the spare textures are common to all.
use std::collections::HashSet;

use super::TerrainResources;
use crate::{
    coords::WorldTileCoords,
    render::{eye_covering::ViewSlotId, resource::Texture},
    terrain::drape_cache::DrapeState,
};

/// A view, `None` for a map drawn without view slots, and a terrain tile it drapes.
pub(crate) type DrapeKey = (Option<ViewSlotId>, WorldTileCoords);

/// A drape texture with the view of its first level, which the drape pass draws into every
/// time the drape is redrawn; the texture's own view spans every level, which an attachment
/// may not.
pub(crate) struct DrapeTexture {
    top_level: wgpu::TextureView,
    texture: Texture,
}

impl DrapeTexture {
    fn new(texture: Texture) -> Self {
        let top_level = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("drape top level"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        Self { top_level, texture }
    }
}

impl TerrainResources {
    /// Chooses the view whose drapes the tile lookups answer with from here on.
    pub(crate) fn show_drapes_of(&mut self, view: Option<ViewSlotId>) {
        self.drape_view = view;
    }

    /// Releases the drapes of a view that draws no more; their textures become spare.
    #[cfg(feature = "headless")]
    pub(crate) fn release_drapes_of(&mut self, view: Option<ViewSlotId>) {
        self.drapes.retain(|(owner, _)| *owner != view);
    }

    fn drape_key(&self, coords: WorldTileCoords) -> DrapeKey {
        (self.drape_view, coords)
    }

    /// Selects the tiered drape texture edge resolution by camera-distance rank:
    /// focus tiles (`rank < 4`) use `self.drape_size` (`1024`), mid-range tiles (`4..12`) use
    /// `<= 512`, and distant horizon or prefetch tiles (`rank >= 12 || prefetch`) use `<= 256`.
    pub fn tier_drape_size(&self, rank: usize, prefetch: bool) -> u32 {
        if self.drape_size == 0 {
            return 0;
        }
        if prefetch || rank >= 12 {
            self.drape_size.min(super::MIN_DRAPE_SIZE)
        } else if rank >= 4 {
            self.drape_size.min(512)
        } else {
            self.drape_size
        }
    }

    /// Gives a view tile a drape texture at full `self.drape_size` and reports what it holds.
    pub fn acquire_drape(
        &mut self,
        device: &wgpu::Device,
        coords: WorldTileCoords,
        fingerprint: u64,
        may_create: bool,
    ) -> DrapeState {
        self.acquire_drape_ranked(device, coords, 0, fingerprint, may_create)
    }

    /// Gives a view tile a distance-LOD tiered drape texture according to its nearest-first `rank`.
    pub fn acquire_drape_ranked(
        &mut self,
        device: &wgpu::Device,
        coords: WorldTileCoords,
        rank: usize,
        fingerprint: u64,
        may_create: bool,
    ) -> DrapeState {
        let format = self.color_format;
        let size = self.tier_drape_size(rank, false);
        let key = self.drape_key(coords);
        self.drapes.acquire_matching(
            key,
            fingerprint,
            may_create,
            |drape| drape.texture.size.width >= size,
            |drape| drape.texture.size.width == size,
            || {
                DrapeTexture::new(Texture::new_mipmapped(
                    Some("drape texture"),
                    device,
                    format,
                    size,
                    size,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                ))
            },
        )
    }

    /// Like [`acquire_drape`](Self::acquire_drape), for a drape prepared ahead of the view.
    pub fn acquire_ahead_drape(
        &mut self,
        device: &wgpu::Device,
        coords: WorldTileCoords,
        fingerprint: u64,
        may_create: bool,
    ) -> DrapeState {
        let format = self.color_format;
        let size = self.tier_drape_size(usize::MAX, true);
        let key = self.drape_key(coords);
        self.drapes.acquire_ahead_matching(
            key,
            fingerprint,
            may_create,
            |drape| drape.texture.size.width >= size,
            |drape| drape.texture.size.width == size,
            || {
                DrapeTexture::new(Texture::new_mipmapped(
                    Some("drape texture"),
                    device,
                    format,
                    size,
                    size,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                ))
            },
        )
    }

    /// Whether the view may take a texture for the tile while holding at most `allowed`: a
    /// tile that has one keeps it.
    pub(crate) fn within_drape_allowance(&self, coords: WorldTileCoords, allowed: usize) -> bool {
        let view = self.drape_view;
        self.drapes.holds(self.drape_key(coords))
            || self.drapes.count(|(owner, _)| *owner == view) < allowed
    }

    /// Leaves a drape acquired this frame undrawn, to be drawn on the next.
    pub fn defer_drape(&mut self, coords: WorldTileCoords, state: DrapeState) {
        let key = self.drape_key(coords);
        self.drapes.defer(key, state);
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

    /// Keeps the requested tile drapes of the view active and parks its other textures with
    /// their content; the drapes of other views stay as they are. Older parked textures are
    /// recycled or dropped according to the cache's spare limits.
    pub fn retain_drapes(&mut self, keep: &HashSet<WorldTileCoords>) {
        let view = self.drape_view;
        self.drapes
            .retain(|(owner, coords)| *owner != view || keep.contains(coords));
    }

    /// Whether a view has drawn the tile's drape, in use or parked after it left the view.
    pub fn holds_shown_drape(&self, coords: WorldTileCoords) -> bool {
        self.drapes.holds_shown(self.drape_key(coords))
    }

    /// Drape texture of a view tile.
    pub fn drape_texture(&self, coords: WorldTileCoords) -> Option<&Texture> {
        self.drapes
            .get(self.drape_key(coords))
            .map(|drape| &drape.texture)
    }

    /// Edge resolution in pixels of the resident drape texture assigned to `coords`, if any.
    pub fn drape_edge_px(&self, coords: WorldTileCoords) -> Option<u32> {
        self.drape_texture(coords).map(|tex| tex.size.width)
    }

    /// Total GPU bytes held by all active, parked, and free mipmapped drape textures.
    pub(super) fn drape_texture_bytes(&self) -> usize {
        self.drapes
            .iter_all_textures()
            .map(|drape| (drape.texture.size.width as usize).pow(2) * 4 * 4 / 3)
            .sum()
    }

    /// Drape texture of a view tile with the view of its first level, the one drawn into.
    pub(crate) fn drape_target(
        &self,
        coords: WorldTileCoords,
    ) -> Option<(&Texture, &wgpu::TextureView)> {
        self.drapes
            .get(self.drape_key(coords))
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

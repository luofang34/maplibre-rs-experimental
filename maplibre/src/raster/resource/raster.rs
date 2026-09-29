use std::collections::HashMap;

use crate::{
    coords::WorldTileCoords,
    render::{resource::Texture, settings::Msaa, tile_view_pattern::HasTile},
    tcs::world::World,
};

/// Raster pipeline and clamp-to-edge sampler with one texture binding per tile coordinate.
/// Bindings are shared with DEM-shaded layers and are not keyed by style source ID.
pub struct RasterResources {
    sampler: wgpu::Sampler,
    msaa: Msaa,
    pipeline: wgpu::RenderPipeline,
    bound_textures: HashMap<WorldTileCoords, (wgpu::BindGroup, u64)>,
    /// Advances whenever a texture is bound, so cached renders of raster tiles can refresh.
    revision: u64,
}

impl RasterResources {
    /// Creates an empty texture registry with linear filtering.
    /// `msaa` controls textures allocated by `create_texture`, not the supplied pipeline.
    pub fn new(msaa: Msaa, device: &wgpu::Device, pipeline: wgpu::RenderPipeline) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            sampler,
            msaa,
            pipeline,
            bound_textures: Default::default(),
            revision: 0,
        }
    }

    /// Wrapping generation advanced by each binding insertion or replacement; removal leaves it unchanged.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Allocates an uninitialized texture with this registry's sample count.
    /// Dimensions are texels; invalid format, usage or dimension combinations fail wgpu validation.
    pub fn create_texture(
        &mut self,
        label: wgpu::Label,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> Texture {
        Texture::new(label, device, format, width, height, self.msaa, usage)
    }

    /// Borrows the current texture/sampler binding, or `None` before upload or after eviction.
    pub fn get_bound_texture(&self, coords: &WorldTileCoords) -> Option<&wgpu::BindGroup> {
        self.bound_textures.get(coords).map(|(binding, _)| binding)
    }

    pub(crate) fn texture_revision(&self, coords: WorldTileCoords) -> Option<u64> {
        self.bound_textures
            .get(&coords)
            .map(|(_, revision)| *revision)
    }

    /// Drops the registry's binding for this tile without advancing the insertion generation.
    pub fn remove_texture(&mut self, coords: WorldTileCoords) {
        self.bound_textures.remove(&coords);
    }

    /// Inserts or replaces the texture/sampler binding at group one of the raster pipeline.
    /// The bind group retains the texture view after the supplied wrapper is dropped.
    pub fn bind_texture(
        &mut self,
        device: &wgpu::Device,
        coords: &WorldTileCoords,
        texture: Texture,
    ) {
        self.revision = self.revision.wrapping_add(1);
        self.bound_textures.insert(
            *coords,
            (
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &self.pipeline.get_bind_group_layout(1),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&texture.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                    label: None,
                }),
                self.revision,
            ),
        );
    }

    /// Borrows the raster pipeline whose group-one layout defines the texture bindings.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

impl HasTile for RasterResources {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.bound_textures.contains_key(&coords)
    }
}

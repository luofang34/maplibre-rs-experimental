use std::collections::HashMap;

use crate::{
    coords::WorldTileCoords,
    raster::RasterSourceId,
    render::{resource::Texture, settings::Msaa, tile_view_pattern::HasTile},
    style::Style,
    tcs::world::World,
};

/// Raster pipeline and sampler with independent bindings for each source and tile.
/// Raster imagery and DEM shading share storage without sharing source identities.
pub struct RasterResources {
    sampler: wgpu::Sampler,
    msaa: Msaa,
    pipeline: wgpu::RenderPipeline,
    bound_textures: HashMap<RasterSourceId, HashMap<WorldTileCoords, (wgpu::BindGroup, u64)>>,
    layer_sources: HashMap<String, RasterSourceId>,
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
            layer_sources: Default::default(),
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
    pub fn get_bound_texture(
        &self,
        source: &RasterSourceId,
        coords: &WorldTileCoords,
    ) -> Option<&wgpu::BindGroup> {
        self.bound_textures
            .get(source)?
            .get(coords)
            .map(|(binding, _)| binding)
    }

    pub(crate) fn texture_revision(&self, layer: &str, coords: WorldTileCoords) -> Option<u64> {
        self.bound_textures
            .get(self.layer_sources.get(layer)?)?
            .get(&coords)
            .map(|(_, revision)| *revision)
    }

    /// Drops every source binding at these coordinates without advancing the insertion generation.
    pub fn remove_texture(&mut self, coords: WorldTileCoords) {
        for textures in self.bound_textures.values_mut() {
            textures.remove(&coords);
        }
    }

    /// Inserts or replaces the texture/sampler binding at group one of the raster pipeline.
    /// The bind group retains the texture view after the supplied wrapper is dropped.
    pub fn bind_texture(
        &mut self,
        device: &wgpu::Device,
        source: &RasterSourceId,
        coords: &WorldTileCoords,
        texture: Texture,
    ) {
        self.revision = self.revision.wrapping_add(1);
        self.bound_textures
            .entry(source.clone())
            .or_default()
            .insert(
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

    /// Drops one source binding while retaining other sources at these coordinates.
    pub fn remove_source_texture(&mut self, source: &RasterSourceId, coords: WorldTileCoords) {
        if let Some(textures) = self.bound_textures.get_mut(source) {
            textures.remove(&coords);
        }
    }

    pub(crate) fn update_layer_sources(&mut self, style: &Style) {
        self.layer_sources.clear();
        for group in crate::io::tile_sources::source_layer_groups(
            style,
            crate::io::tile_sources::TileKind::Raster,
        ) {
            let source = RasterSourceId::new(group.source_name);
            for layer in group.layers {
                self.layer_sources.insert(layer.id, source.clone());
            }
        }
    }

    pub(crate) fn layer_source(&self, layer: &str) -> Option<&RasterSourceId> {
        self.layer_sources.get(layer)
    }

    pub(crate) fn layer_texture(
        &self,
        layer: &str,
        coords: &WorldTileCoords,
    ) -> Option<&wgpu::BindGroup> {
        self.get_bound_texture(self.layer_source(layer)?, coords)
    }

    /// Borrows the raster pipeline whose group-one layout defines the texture bindings.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

impl HasTile for RasterResources {
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        _world: &World,
    ) -> bool {
        self.get_bound_texture(source, &coords).is_some()
    }

    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.bound_textures
            .values()
            .any(|textures| textures.contains_key(&coords))
    }
}

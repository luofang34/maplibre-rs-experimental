//! Cached texture bindings for terrain surfaces and their symbols.
use super::*;

impl TerrainResources {
    /// Binds the uniform block window, a DEM texture and a drape texture for one tile.
    pub fn create_bind_group(
        &self,
        device: &wgpu::Device,
        dem: &Texture,
        drape: &Texture,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain tile"),
            layout: &self.pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniform_buffer,
                        offset: 0,
                        size: NonZeroU64::new(std::mem::size_of::<TerrainTileUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dem.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&drape.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Reuses bindings when both textures are unchanged. No drape source binds an inert
    /// texture for the shader's background surface; an invalid named source returns `None`.
    pub fn tile_bind_group(
        &mut self,
        device: &wgpu::Device,
        dem: Option<WorldTileCoords>,
        drape_source: Option<WorldTileCoords>,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let key = {
            let dem = self.dem_texture(dem);
            let drape = match drape_source {
                Some(source) => self.drape_texture(source)?,
                None => &self.empty_dem,
            };
            (dem.texture.global_id(), drape.texture.global_id())
        };
        if let Some(group) = self.bind_groups.get(&key) {
            return Some(Arc::clone(group));
        }
        let group = {
            let dem = self.dem_texture(dem);
            let drape = match drape_source {
                Some(source) => self.drape_texture(source)?,
                None => &self.empty_dem,
            };
            Arc::new(self.create_bind_group(device, dem, drape))
        };
        self.bind_groups.insert(key, Arc::clone(&group));
        Some(group)
    }
}

//! Cached texture bindings for terrain surfaces and their symbols, including edge heights.
use super::*;
use crate::render::resource::share_gpu;

impl TerrainResources {
    /// Bind group layout of group one: uniforms, DEM texture, drape texture, sampler, edge heights.
    pub fn bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
        vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(
                        std::mem::size_of::<TerrainTileUniforms>() as u64
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::VERTEX,
                // A texture rather than a storage buffer: WebGL2 has no vertex storage.
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
        ]
    }

    /// Binds the uniform block window, a DEM texture, a drape texture, and the edge heights for one tile.
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
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.edge_heights.view),
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
            (dem.texture.clone(), drape.texture.clone())
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
            share_gpu(self.create_bind_group(device, dem, drape))
        };
        self.bind_groups.insert(key, Arc::clone(&group));
        Some(group)
    }
}

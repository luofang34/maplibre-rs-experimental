//! The terrain bind group symbols take when no terrain tile stands under them.

use std::num::NonZeroU64;

use crate::{
    render::{resource::Texture, settings::Msaa},
    terrain::resources::TerrainTileUniforms,
};

/// A flat stand-in for the terrain tile bind group: zeroed uniforms and blank textures, so the
/// symbol shader samples an elevation of zero.
pub struct SymbolTerrainFallback {
    bind_group: wgpu::BindGroup,
}

impl SymbolTerrainFallback {
    /// Builds the stand-in against group 2 of the symbol pipeline.
    pub fn new(device: &wgpu::Device, pipeline: &wgpu::RenderPipeline) -> Self {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flat symbol terrain uniforms"),
            size: std::mem::size_of::<TerrainTileUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let texture = |label| {
            Texture::new(
                Some(label),
                device,
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                1,
                Msaa { samples: 1 },
                wgpu::TextureUsages::TEXTURE_BINDING,
            )
        };
        let dem = texture("flat symbol DEM");
        let drape = texture("flat symbol drape");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flat symbol terrain"),
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniforms,
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
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Self { bind_group }
    }

    /// The stand-in bind group.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

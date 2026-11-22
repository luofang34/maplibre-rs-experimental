//! Cached texture bindings for terrain surfaces and their symbols, including edge heights.
use super::*;
use crate::{
    render::resource::share_gpu,
    terrain::mesh::{create_terrain_mesh, TERRAIN_MESH_SIZE},
};

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
        let key = (
            self.dem_id(dem),
            match drape_source {
                Some(source) => self.drape_id(source)?,
                None => self.empty_dem_id,
            },
        );
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

pub(super) struct TerrainGpuInit {
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) index_count: u32,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) sampler: wgpu::Sampler,
    pub(super) empty_dem: Texture,
}

pub(super) fn create_shared_gpu_resources(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
) -> TerrainGpuInit {
    let mesh = create_terrain_mesh(TERRAIN_MESH_SIZE);
    let vertex_buffer = queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("terrain mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );
    let index_buffer = queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("terrain mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        },
    );
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain tile uniforms"),
        size: UNIFORM_STRIDE * UNIFORM_CAPACITY,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        // Drapes are seen at grazing angles at the horizon; anisotropy keeps their
        // detail along the view direction while the mip levels stop the shimmer.
        anisotropy_clamp: 16,
        ..Default::default()
    });
    // A fresh texture reads as zero, which decodes to sea level with a zero unpack vector.
    let empty_dem = Texture::new(
        Some("empty DEM"),
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
        1,
        Msaa { samples: 1 },
        wgpu::TextureUsages::TEXTURE_BINDING,
    );
    TerrainGpuInit {
        vertex_buffer,
        index_buffer,
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        sampler,
        empty_dem,
    }
}

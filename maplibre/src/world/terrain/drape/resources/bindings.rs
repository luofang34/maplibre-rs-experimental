//! Cached texture bindings for terrain surfaces and their symbols, including edge heights.
use super::*;
use crate::{
    render::resource::share_gpu,
    terrain::mesh::{create_terrain_mesh, TERRAIN_MESH_SIZE},
};

fn uniform_buffer_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(std::mem::size_of::<TerrainTileUniforms>() as u64),
        },
        count: None,
    }
}

fn texture_layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    filterable: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable },
        },
        count: None,
    }
}

fn texture_binding_entry<'a>(
    binding: u32,
    view: &'a wgpu::TextureView,
) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

impl TerrainResources {
    /// Bind group layout of group one: uniforms, DEM texture, drape texture, sampler, edge heights.
    pub fn bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
        vec![
            uniform_buffer_layout_entry(0),
            texture_layout_entry(1, wgpu::ShaderStages::VERTEX, true),
            texture_layout_entry(2, wgpu::ShaderStages::FRAGMENT, true),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // A texture rather than a storage buffer: WebGL2 has no vertex storage.
            texture_layout_entry(4, wgpu::ShaderStages::VERTEX, false),
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
                texture_binding_entry(1, dem),
                texture_binding_entry(2, drape),
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                texture_binding_entry(4, &self.edge_heights.view),
            ],
        })
    }

    fn resolve_bind_group_textures(
        &self,
        dem: Option<WorldTileCoords>,
        drape_source: Option<WorldTileCoords>,
    ) -> Option<(&Texture, &Texture)> {
        let dem = self.dem_texture(dem);
        let drape = match drape_source {
            Some(source) => self.drape_texture(source)?,
            None => &self.empty_dem,
        };
        Some((dem, drape))
    }

    /// Reuses bindings using an already-resolved DEM texture ID, avoiding a duplicate DEM map lookup.
    pub(crate) fn tile_bind_group_with_dem_id(
        &mut self,
        device: &wgpu::Device,
        dem: Option<WorldTileCoords>,
        dem_id: Option<u64>,
        drape_source: Option<WorldTileCoords>,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let key = (
            dem_id.unwrap_or_else(|| self.empty_dem_id()),
            match drape_source {
                Some(source) => self.drape_id(source)?,
                None => self.empty_dem_id,
            },
        );
        if let Some(group) = self.bind_groups.get(&key) {
            return Some(Arc::clone(group));
        }
        let group = {
            let (dem, drape) = self.resolve_bind_group_textures(dem, drape_source)?;
            share_gpu(self.create_bind_group(device, dem, drape))
        };
        self.bind_groups.insert(key, Arc::clone(&group));
        Some(group)
    }

    /// Reuses bindings when both textures are unchanged. No drape source binds an inert
    /// texture for the shader's background surface; an invalid named source returns `None`.
    pub fn tile_bind_group(
        &mut self,
        device: &wgpu::Device,
        dem: Option<WorldTileCoords>,
        drape_source: Option<WorldTileCoords>,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let dem_id = dem.map(|coords| self.dem_id(Some(coords)));
        self.tile_bind_group_with_dem_id(device, dem, dem_id, drape_source)
    }

    /// Draws every terrain tile queued for this frame, writing depth.
    pub(crate) fn draw<'w>(
        &'w self,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        projection: &'w crate::render::projection::ProjectionGpuResources,
    ) {
        if self.draws.is_empty() {
            tracing::trace!("no terrain draws queued");
            return;
        }
        tracing::trace!(draws = self.draws.len(), "drawing terrain");
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(
            0,
            projection.bind_group_for(crate::render::projection::ProjectionBinding::View),
            &[],
        );
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for draw in &self.draws {
            pass.set_bind_group(1, draw.bind_group.as_ref(), &[draw.uniform_offset]);
            pass.draw_indexed(0..self.index_count(), 0, 0..1);
        }
    }

    /// Number of queued terrain draws in the current frame.
    #[inline]
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Invokes `f` with `(coords, bind_group, uniform_offset)` for every queued terrain draw.
    #[inline]
    pub fn try_for_each_draw<E>(
        &self,
        mut f: impl FnMut(WorldTileCoords, &wgpu::BindGroup, u32) -> Result<(), E>,
    ) -> Result<(), E> {
        for draw in &self.draws {
            f(draw.coords, draw.bind_group.as_ref(), draw.uniform_offset)?;
        }
        Ok(())
    }

    /// Replays per-tile terrain uniforms using the reusable scratch staging buffer.
    pub(crate) fn replay_uniforms(
        &mut self,
        queue: &crate::render::upload_queue::UploadQueue,
        frame_uniforms: &[(WorldTileCoords, TerrainTileUniforms)],
    ) {
        self.replay_uniforms_scratch.clear();
        self.replay_uniforms_scratch
            .extend(frame_uniforms.iter().map(|(_, uniform)| *uniform));
        write_uniforms_into(
            (&self.uniform_buffer, self.edge_heights.capacity()),
            &mut self.last_uploaded_uniforms,
            queue,
            &self.replay_uniforms_scratch,
        );
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

fn create_terrain_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
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
    })
}

fn create_empty_dem_texture(device: &wgpu::Device) -> Texture {
    // A fresh texture reads as zero, which decodes to sea level with a zero unpack vector.
    Texture::new(
        Some("empty DEM"),
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
        1,
        Msaa { samples: 1 },
        wgpu::TextureUsages::TEXTURE_BINDING,
    )
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
    let sampler = create_terrain_sampler(device);
    let empty_dem = create_empty_dem_texture(device);
    TerrainGpuInit {
        vertex_buffer,
        index_buffer,
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        sampler,
        empty_dem,
    }
}

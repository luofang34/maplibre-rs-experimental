//! GPU resources shared by the drape pass and the terrain draw.

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
};

use bytemuck_derive::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::{
    coords::WorldTileCoords,
    render::{
        resource::{MipmapGenerator, Texture},
        settings::Msaa,
    },
    terrain::{
        dem::DemTile,
        drape_cache::{DrapeCache, DrapeState},
        mesh::{create_terrain_mesh, TERRAIN_MESH_SIZE},
    },
};

/// Edge length in pixels of one drape texture; twice the tile size, as GL JS `qualityFactor`.
pub const DRAPE_SIZE: u32 = 1024;
/// Byte stride between per-tile uniform blocks, the WebGPU dynamic offset alignment.
pub const UNIFORM_STRIDE: u64 = 512;
// A block that outgrows its stride would overwrite the next tile's; the stride is a
// multiple of the 256-byte offset alignment every backend accepts.
const _: () = assert!(std::mem::size_of::<TerrainTileUniforms>() as u64 <= UNIFORM_STRIDE);
/// Largest number of terrain tiles drawn in one frame.
const UNIFORM_CAPACITY: u64 = 1024;

/// Per-tile inputs of the terrain shaders, one block per drawn tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TerrainTileUniforms {
    /// GPU view projection times the tile transform.
    pub transform: [[f32; 4]; 4],
    /// Maps tile coordinates to unit coordinates of the sampled DEM tile.
    pub dem_matrix: [[f32; 4]; 4],
    /// Maps the tile's unit coordinates into the drape texture drawn for it, which is an
    /// ancestor's while the tile's own is not drawn yet.
    pub drape_matrix: [[f32; 4]; 4],
    /// Mercator offset and scale of the tile for the globe path.
    pub tile_mercator_coords: [f32; 4],
    /// Channel factors and base shift decoding DEM pixels to metres.
    pub dem_unpack: [f32; 4],
    /// Samples per DEM tile edge without the border.
    pub dem_dim: f32,
    /// Elevation multiplier.
    pub exaggeration: f32,
    /// Distance in metres the skirt vertices drop below the surface.
    pub skirt_length: f32,
    /// Keeps the block a multiple of sixteen bytes.
    pub padding: f32,
    /// Premultiplied fog colour.
    pub fog_color: [f32; 4],
    /// Premultiplied horizon colour the fog blends into.
    pub horizon_color: [f32; 4],
    /// Fog depth range in pixels, then the ground blend and the horizon blend.
    pub fog_range: [f32; 4],
    /// Fog opacity at the current pitch and whether the map is a globe.
    pub fog_opacity: [f32; 4],
}

/// The fog of a frame, shared by every terrain tile, as GL JS `terrainUniformValues` takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerrainFog {
    /// Premultiplied fog colour.
    pub fog_color: [f32; 4],
    /// Premultiplied horizon colour.
    pub horizon_color: [f32; 4],
    /// Distances in pixels the fog depth runs between.
    pub near: f32,
    /// Far end of the fog depth in pixels.
    pub far: f32,
    /// Fog depth at which the ground starts to take the fog colour.
    pub ground_blend: f32,
    /// Fog depth at which the fog colour turns into the horizon colour.
    pub horizon_blend: f32,
    /// Opacity of the fog at the current pitch.
    pub opacity: f32,
    /// Whether the map is drawn as a globe, where GL JS draws no fog.
    pub globe: bool,
}

/// One terrain tile ready to draw.
pub struct TerrainDraw {
    /// The terrain tile drawn.
    pub coords: WorldTileCoords,
    /// Uniform block, DEM texture and drape texture of the tile.
    pub bind_group: wgpu::BindGroup,
    /// Dynamic offset of the tile's uniform block.
    pub uniform_offset: u32,
}

/// Attachments every drape pass renders into before resolving to a tile texture.
pub struct DrapeScratch {
    /// Multisampled color target, or `None` when the layer pipelines are single-sampled.
    pub color: Option<Texture>,
    /// Depth-stencil target matching the layer pipelines.
    pub depth_stencil: Texture,
}

/// Pipeline, mesh, textures and per-frame draws of the terrain.
pub struct TerrainResources {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    mipmaps: MipmapGenerator,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    dem_textures: HashMap<WorldTileCoords, (Texture, u32)>,
    empty_dem: Texture,
    drapes: DrapeCache<Texture>,
    drape_scratch: Option<DrapeScratch>,
    draws: Vec<TerrainDraw>,
    msaa: Msaa,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
}

impl TerrainResources {
    /// Bind group layout of group one: uniforms, DEM texture, drape texture, sampler.
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
        ]
    }

    /// Creates the shared mesh, sampler and uniform storage for an initialized pipeline.
    pub fn new(
        device: &wgpu::Device,
        pipeline: wgpu::RenderPipeline,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        msaa: Msaa,
    ) -> Self {
        let mesh = create_terrain_mesh(TERRAIN_MESH_SIZE);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
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
            mipmap_filter: wgpu::FilterMode::Linear,
            // Drapes are seen at grazing angles at the horizon; anisotropy keeps their
            // detail along the view direction while the mip levels stop the shimmer.
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let mipmaps = MipmapGenerator::new(device, color_format);
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
        Self {
            pipeline,
            sampler,
            mipmaps,
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            uniform_buffer,
            dem_textures: HashMap::new(),
            empty_dem,
            drapes: DrapeCache::default(),
            drape_scratch: None,
            draws: Vec::new(),
            msaa,
            color_format,
            depth_format,
        }
    }

    /// Terrain render pipeline.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    /// Shared grid vertex buffer.
    pub fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    /// Shared grid index buffer, 32-bit indices.
    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }

    /// Number of indices in the shared grid.
    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    /// Sample count the drape passes and terrain pipeline were built for.
    pub fn msaa(&self) -> Msaa {
        self.msaa
    }

    /// Whether a DEM tile has been uploaded.
    pub fn has_dem_texture(&self, coords: WorldTileCoords) -> bool {
        self.dem_textures.contains_key(&coords)
    }

    /// Revision of the uploaded copy of a DEM tile, if any.
    pub fn dem_revision(&self, coords: WorldTileCoords) -> Option<u32> {
        self.dem_textures
            .get(&coords)
            .map(|(_, revision)| *revision)
    }

    /// Uploads the bordered pixels of a decoded DEM tile, reusing its texture across revisions.
    pub fn upload_dem(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        coords: WorldTileCoords,
        dem: &DemTile,
        revision: u32,
    ) {
        let reusable = self
            .dem_textures
            .remove(&coords)
            .map(|(texture, _)| texture)
            .filter(|texture| texture.size.width == dem.stride());
        let texture = reusable.unwrap_or_else(|| {
            Texture::new(
                Some("DEM tile"),
                device,
                wgpu::TextureFormat::Rgba8Unorm,
                dem.stride(),
                dem.stride(),
                Msaa { samples: 1 },
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            )
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            dem.pixels(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * dem.stride()),
                rows_per_image: Some(dem.stride()),
            },
            texture.size,
        );
        self.dem_textures.insert(coords, (texture, revision));
    }

    /// Releases the DEM texture of a tile that left the store.
    pub fn drop_dem(&mut self, coords: WorldTileCoords) {
        self.dem_textures.remove(&coords);
    }

    /// DEM texture of a tile, or the flat stand-in while it loads.
    pub fn dem_texture(&self, coords: Option<WorldTileCoords>) -> &Texture {
        coords
            .and_then(|coords| self.dem_textures.get(&coords))
            .map_or(&self.empty_dem, |(texture, _)| texture)
    }

    /// Gives a view tile a drape texture and reports what it holds.
    pub fn acquire_drape(
        &mut self,
        device: &wgpu::Device,
        coords: WorldTileCoords,
        fingerprint: u64,
    ) -> DrapeState {
        let format = self.color_format;
        self.drapes.acquire(coords, fingerprint, || {
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
    pub fn defer_drape(&mut self, coords: WorldTileCoords) {
        self.drapes.defer(coords);
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

    /// Releases the drape textures of tiles that left the view for reuse.
    /// Drape textures held for tiles, and parked or waiting in the free list.
    pub fn drape_counts(&self) -> (usize, usize) {
        (
            self.drapes.len(),
            self.drapes.parked_len() + self.drapes.free_len(),
        )
    }

    /// DEM textures resident on the GPU.
    pub fn dem_texture_count(&self) -> usize {
        self.dem_textures.len()
    }

    /// Bytes of drape textures, held and free, and of DEM textures.
    pub fn texture_bytes(&self) -> (usize, usize) {
        let (held, free) = self.drape_counts();
        // Four bytes a texel, and a mip chain adds a third.
        let drape = (DRAPE_SIZE as usize).pow(2) * 4 * 4 / 3;
        let dem = self
            .dem_textures
            .values()
            .map(|(texture, _)| (texture.size.width * texture.size.height * 4) as usize)
            .sum();
        ((held + free) * drape, dem)
    }

    pub fn retain_drapes(&mut self, keep: &HashSet<WorldTileCoords>) {
        self.drapes.retain(keep);
    }

    /// Drape texture of a view tile.
    pub fn drape_texture(&self, coords: WorldTileCoords) -> Option<&Texture> {
        self.drapes.get(coords)
    }

    /// Creates the scratch attachments used by every drape pass.
    pub fn ensure_scratch(&mut self, device: &wgpu::Device) {
        if self.drape_scratch.is_some() {
            return;
        }
        let color = self.msaa.is_multisampling().then(|| {
            Texture::new(
                Some("drape multisampled color"),
                device,
                self.color_format,
                DRAPE_SIZE,
                DRAPE_SIZE,
                self.msaa,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth_stencil = Texture::new(
            Some("drape depth stencil"),
            device,
            self.depth_format,
            DRAPE_SIZE,
            DRAPE_SIZE,
            self.msaa,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        self.drape_scratch = Some(DrapeScratch {
            color,
            depth_stencil,
        });
    }

    /// Scratch attachments of the drape passes.
    pub fn scratch(&self) -> Option<&DrapeScratch> {
        self.drape_scratch.as_ref()
    }

    /// Writes the per-tile uniform blocks and returns how many fit.
    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &[TerrainTileUniforms]) -> usize {
        let count = uniforms.len().min(UNIFORM_CAPACITY as usize);
        if count < uniforms.len() {
            tracing::warn!(
                requested = uniforms.len(),
                capacity = UNIFORM_CAPACITY,
                "more terrain tiles than uniform blocks; distant tiles are skipped"
            );
        }
        let mut bytes = vec![0_u8; count * UNIFORM_STRIDE as usize];
        for (index, block) in uniforms.iter().take(count).enumerate() {
            let start = index * UNIFORM_STRIDE as usize;
            let raw = bytemuck::bytes_of(block);
            bytes[start..start + raw.len()].copy_from_slice(raw);
        }
        queue.write_buffer(&self.uniform_buffer, 0, &bytes);
        count
    }

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

    /// Replaces this frame's terrain draws.
    pub fn set_draws(&mut self, draws: Vec<TerrainDraw>) {
        self.draws = draws;
    }

    /// Terrain draws of the current frame.
    pub fn draws(&self) -> &[TerrainDraw] {
        &self.draws
    }

    /// The draw of a terrain tile this frame, whose bind group also elevates symbols on it.
    pub fn draw_for(&self, coords: WorldTileCoords) -> Option<&TerrainDraw> {
        self.draws.iter().find(|draw| draw.coords == coords)
    }
}

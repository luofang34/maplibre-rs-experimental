//! GPU resources shared by the drape pass and the terrain draw.

use std::{
    num::NonZeroU64,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use bytemuck_derive::{Pod, Zeroable};
use rustc_hash::FxHashMap;

use crate::{
    coords::WorldTileCoords,
    render::{
        resource::{MipmapGenerator, Texture},
        settings::{Msaa, TerrainDrapeMode},
    },
    terrain::drape_cache::DrapeCache,
};

mod bindings;
mod drapes;
mod edge_heights;
mod elevation_textures;
#[cfg(test)]
mod tests;

use elevation_textures::DemTexture;

static NEXT_TERRAIN_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_texture_id() -> u64 {
    NEXT_TERRAIN_TEXTURE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Edge length in pixels of one drape texture; twice a 512 px tile (`1024 × 1024`).
pub const DRAPE_SIZE: u32 = 1024;
/// Smallest drape edge accepted; below it the ground is too coarse to read.
pub const MIN_DRAPE_SIZE: u32 = 256;
/// Byte stride between per-tile uniform blocks, the WebGPU dynamic offset alignment.
pub const UNIFORM_STRIDE: u64 = 512;
// A block that outgrows its stride would overwrite the next tile's; the stride is a
// multiple of the 256-byte offset alignment every backend accepts.
const _: () = assert!(std::mem::size_of::<TerrainTileUniforms>() as u64 <= UNIFORM_STRIDE);
const _: () = assert!(std::mem::size_of::<TerrainTileUniforms>() == 512);
/// Largest number of terrain tiles drawn in one frame, matching `globe_view_region` / `MAX_N_TILES`.
const UNIFORM_CAPACITY: u64 = 512;

/// Per-tile inputs of the terrain shaders, one block per drawn tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TerrainTileUniforms {
    /// GPU view projection times the tile transform.
    pub transform: [[f32; 4]; 4],
    /// Tile-centred globe projection, with the large translation resolved in f64.
    pub globe_transform: [[f32; 4]; 4],
    /// Centre latitude sine/cosine, radians per tile unit, and radius; zero radius disables it.
    pub globe_origin: [f32; 4],
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
    /// Strength of directional relief lighting, zero for unlit map styles.
    pub relief_strength: f32,
    /// Premultiplied fog colour.
    pub fog_color: [f32; 4],
    /// Premultiplied horizon colour the fog blends into.
    pub horizon_color: [f32; 4],
    /// Fog depth range (metres for external eyes, otherwise pixels), then blend thresholds.
    pub fog_range: [f32; 4],
    /// Fog opacity, globe flag, fallback surface flag, and radial fog flag.
    pub fog_opacity: [f32; 4],
    /// Background color while a tile has no valid drape texture.
    pub surface_color: [f32; 4],
    /// Tile origin relative to the eye in metres, and metres per tile coordinate.
    pub fog_position: [f32; 4],
    /// Direction towards the sun in globe world axes, with w 1 while a host has set one; all
    /// zero leaves every tile lit as day.
    pub daylight: [f32; 4],
    /// Sines of the solar elevations ending night and starting day, then the night brightness.
    pub twilight: [f32; 4],
    /// Direction towards the Moon in globe world axes, with w the light it adds overhead;
    /// all zero without a host Moon.
    pub moonlight: [f32; 4],
    /// Procedural DEM relief controls: `[enabled, hypsometric, slope_shading, reference_alt_m]`.
    pub procedural_relief: [f32; 4],
    /// Procedural DEM grid & TAWS controls: `[grid_spacing_m, grid_opacity, taws_enabled, tile_span_m]`.
    pub procedural_grid: [f32; 4],
    /// Edge buffer index in `[0]`, and packed RGBA8 north/south polar cap colours in `[1]`/`[2]`.
    pub edge_index: [u32; 4],
}

/// The fog of a frame, shared by every terrain tile, as GL JS `terrainUniformValues` takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerrainFog {
    /// Premultiplied fog colour.
    pub fog_color: [f32; 4],
    /// Premultiplied horizon colour.
    pub horizon_color: [f32; 4],
    /// Near fog distance in metres for external eyes, otherwise pixels.
    pub near: f32,
    /// Far fog distance in the same units as `near`.
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
    /// Uniform block, DEM texture and drape texture of the tile, shared with the cache that
    /// keeps it for the next frame.
    pub bind_group: Arc<wgpu::BindGroup>,
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
    mipmaps: Option<MipmapGenerator>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    last_uploaded_uniforms: Option<(u64, u64, usize)>,
    pub(crate) replay_uniforms_scratch: Vec<TerrainTileUniforms>,
    dem_textures: FxHashMap<WorldTileCoords, DemTexture>,
    dem_frame_tick: u64,
    empty_dem: Texture,
    empty_dem_id: u64,
    drapes: DrapeCache<drapes::DrapeTexture, drapes::DrapeKey>,
    /// The view whose drapes the tile lookups below answer with.
    drape_view: Option<crate::render::eye_covering::ViewSlotId>,
    drape_retain_state: FxHashMap<
        Option<crate::render::eye_covering::ViewSlotId>,
        (Vec<WorldTileCoords>, usize, bool),
    >,
    drape_scratches: Vec<(u32, DrapeScratch)>,
    draws: Vec<TerrainDraw>,
    /// Bind groups keyed by monotonic `(dem_id, drape_id)` texture IDs, avoiding
    /// `wgpu::Texture` clones on hit and stale retention across drops.
    bind_groups: FxHashMap<(u64, u64), Arc<wgpu::BindGroup>>,
    edge_heights: edge_heights::EdgeHeightTexture,
    msaa: Msaa,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    drape_size: u32,
    drape_mode: TerrainDrapeMode,
}

impl TerrainResources {
    /// Creates the shared mesh, sampler and uniform storage for an initialized pipeline.
    pub fn new(
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        pipeline: wgpu::RenderPipeline,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        msaa: Msaa,
        drape_size: u32,
    ) -> Self {
        Self::new_with_mode(
            device,
            queue,
            pipeline,
            color_format,
            depth_format,
            msaa,
            TerrainDrapeMode::Draped {
                edge_px: drape_size,
            },
        )
    }

    /// Creates terrain resources configured for `drape_mode`, omitting `MipmapGenerator` and
    /// drape allocations when [`TerrainDrapeMode::ProceduralRelief`] is active.
    pub fn new_with_mode(
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        pipeline: wgpu::RenderPipeline,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        msaa: Msaa,
        drape_mode: TerrainDrapeMode,
    ) -> Self {
        let max_dim = device.limits().max_texture_dimension_2d.max(MIN_DRAPE_SIZE);
        let (drape_size, drape_mode, mipmaps) = match drape_mode {
            TerrainDrapeMode::Draped { edge_px } => {
                let clamped = edge_px.clamp(MIN_DRAPE_SIZE, max_dim);
                (
                    clamped,
                    TerrainDrapeMode::Draped { edge_px: clamped },
                    Some(MipmapGenerator::new(device, color_format)),
                )
            }
            TerrainDrapeMode::ProceduralRelief(style) => {
                (0, TerrainDrapeMode::ProceduralRelief(style), None)
            }
        };
        let gpu = bindings::create_shared_gpu_resources(device, queue);
        let edge_heights = edge_heights::EdgeHeightTexture::new(device);
        Self {
            pipeline,
            sampler: gpu.sampler,
            mipmaps,
            vertex_buffer: gpu.vertex_buffer,
            index_buffer: gpu.index_buffer,
            index_count: gpu.index_count,
            uniform_buffer: gpu.uniform_buffer,
            last_uploaded_uniforms: None,
            replay_uniforms_scratch: Vec::new(),
            dem_textures: FxHashMap::default(),
            dem_frame_tick: 1,
            empty_dem: gpu.empty_dem,
            empty_dem_id: next_texture_id(),
            drapes: DrapeCache::default(),
            drape_view: None,
            drape_retain_state: FxHashMap::default(),
            drape_scratches: Vec::new(),
            draws: Vec::new(),
            bind_groups: FxHashMap::default(),
            edge_heights,
            msaa,
            color_format,
            depth_format,
            drape_size,
            drape_mode,
        }
    }

    /// Active terrain drape mode.
    pub fn drape_mode(&self) -> TerrainDrapeMode {
        self.drape_mode
    }

    /// Updates the active [`TerrainDrapeMode`], immediately releasing `DrapeScratch` and
    /// `DrapeCache` textures when switching to [`TerrainDrapeMode::ProceduralRelief`] or resizing.
    pub fn set_drape_mode(&mut self, device: &wgpu::Device, mode: TerrainDrapeMode) {
        let max_dim = device.limits().max_texture_dimension_2d.max(MIN_DRAPE_SIZE);
        match mode {
            TerrainDrapeMode::ProceduralRelief(style) => {
                self.drape_mode = TerrainDrapeMode::ProceduralRelief(style);
                self.drape_size = 0;
                self.drape_scratches.clear();
                self.drapes = DrapeCache::default();
                self.drape_retain_state.clear();
                self.bind_groups.clear();
            }
            TerrainDrapeMode::Draped { edge_px } => {
                let clamped = edge_px.clamp(MIN_DRAPE_SIZE, max_dim);
                if self.drape_size != clamped {
                    self.drape_size = clamped;
                    self.drape_scratches.clear();
                    self.drapes = DrapeCache::default();
                    self.drape_retain_state.clear();
                    self.bind_groups.clear();
                }
                self.drape_mode = TerrainDrapeMode::Draped { edge_px: clamped };
                if self.mipmaps.is_none() {
                    self.mipmaps = Some(MipmapGenerator::new(device, self.color_format));
                }
            }
        }
    }

    /// Edge in pixels of each drape texture and of the drape pass's attachments.
    pub fn drape_size(&self) -> u32 {
        self.drape_size
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

    /// Bytes of drape textures, held and free, and of DEM textures.
    pub fn texture_bytes(&self) -> (usize, usize) {
        // Four bytes a texel, and a mip chain adds a third.
        let drape = self.drape_texture_bytes();
        let dem = self
            .dem_textures
            .values()
            .map(|dem| (dem.texture.size.width * dem.texture.size.height * 4) as usize)
            .sum();
        (drape, dem)
    }

    /// Creates the scratch attachments used by drape passes at the configured maximum edge size.
    pub fn ensure_scratch(&mut self, device: &wgpu::Device) {
        self.ensure_scratch_for_size(device, self.drape_size);
    }

    /// Creates per-tier scratch attachments matching `edge_px` when not yet allocated.
    pub fn ensure_scratch_for_size(&mut self, device: &wgpu::Device, edge_px: u32) {
        if self.drape_mode.is_procedural_relief()
            || edge_px == 0
            || self.scratch_for_size(edge_px).is_some()
        {
            return;
        }
        let color = self.msaa.is_multisampling().then(|| {
            Texture::new(
                Some("drape multisampled color"),
                device,
                self.color_format,
                edge_px,
                edge_px,
                self.msaa,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth_stencil = Texture::new(
            Some("drape depth stencil"),
            device,
            self.depth_format,
            edge_px,
            edge_px,
            self.msaa,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        self.drape_scratches.push((
            edge_px,
            DrapeScratch {
                color,
                depth_stencil,
            },
        ));
    }

    /// Scratch attachments matching `edge_px` for a tiered drape pass.
    pub fn scratch_for_size(&self, edge_px: u32) -> Option<&DrapeScratch> {
        self.drape_scratches
            .iter()
            .find(|(size, _)| *size == edge_px)
            .map(|(_, scratch)| scratch)
    }

    /// Scratch attachments of the configured primary drape size.
    pub fn scratch(&self) -> Option<&DrapeScratch> {
        self.scratch_for_size(self.drape_size)
    }

    /// Writes the per-tile uniform blocks when their bytes changed and returns how many fit.
    pub fn write_uniforms(
        &mut self,
        queue: &crate::render::upload_queue::UploadQueue,
        uniforms: &[TerrainTileUniforms],
    ) -> usize {
        let capacity = (UNIFORM_CAPACITY as usize).min(self.edge_heights.capacity());
        let count = uniforms.len().min(capacity);
        if count < uniforms.len() {
            tracing::warn!(
                requested = uniforms.len(),
                capacity,
                "more terrain tiles than uniform blocks; distant tiles are skipped"
            );
        }
        if count > 0
            && let Some(slice) = uniforms.get(..count)
        {
            let bytes = bytemuck::cast_slice(slice);
            let digest = uniform_bytes_digest(bytes);
            if self.last_uploaded_uniforms != Some(digest) {
                queue.write_buffer(&self.uniform_buffer, 0, bytes);
                self.last_uploaded_uniforms = Some(digest);
            }
        }
        count
    }

    /// Replaces this frame's terrain draws.
    pub fn set_draws(&mut self, draws: Vec<TerrainDraw>) {
        self.draws = draws;
        // A bind group keeps its textures alive; only the ones this frame draws with stay.
        if self.bind_groups.len() <= self.draws.len()
            && self.bind_groups.values().all(|g| Arc::strong_count(g) > 1)
        {
            return;
        }
        let used: rustc_hash::FxHashSet<usize> = self
            .draws
            .iter()
            .map(|draw| Arc::as_ptr(&draw.bind_group) as usize)
            .collect();
        self.bind_groups
            .retain(|_, group| used.contains(&(Arc::as_ptr(group) as usize)));
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

impl Drop for TerrainResources {
    fn drop(&mut self) {
        self.draws.clear();
        self.bind_groups.clear();
        self.vertex_buffer.destroy();
        self.index_buffer.destroy();
        self.uniform_buffer.destroy();
    }
}

fn uniform_bytes_digest(bytes: &[u8]) -> (u64, u64, usize) {
    let mut h1 = 0x517c_c1b7_2722_0a95_u64;
    let mut h2 = 0xcbf2_9ce4_8422_2325_u64;
    let (chunks, rem) = bytes.as_chunks::<8>();
    for &chunk in chunks {
        let word = u64::from_le_bytes(chunk);
        h1 = h1
            .rotate_left(5)
            .wrapping_add(word)
            .wrapping_mul(0x517c_c1b7_2722_0a95);
        h2 = (h2 ^ word).wrapping_mul(0x0100_0000_01b3).rotate_right(7);
    }
    for &byte in rem {
        h1 = h1
            .rotate_left(5)
            .wrapping_add(u64::from(byte))
            .wrapping_mul(0x517c_c1b7_2722_0a95);
        h2 = (h2 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    (h1, h2, bytes.len())
}

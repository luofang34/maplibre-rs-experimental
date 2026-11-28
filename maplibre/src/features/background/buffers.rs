//! Persistent GPU buffers and metadata construction for background, sky, and atmosphere rendering.

mod globe_uniform;
mod scratch;
mod sky;

use cgmath::{Matrix3, Matrix4, SquareMatrix, Vector3};
#[cfg(test)]
pub(crate) use globe_uniform::globe_layer_bind_group_layout;
use globe_uniform::GlobeLayerUniformPool;
use scratch::BackgroundScratch;
pub use sky::{sky_metadata, sky_metadata_with_camera};

use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    render::{
        shaders::{
            AtmosphereLayerMetadata, BackgroundLayerMetadata, ShaderTileMetadata, SkyLayerMetadata,
            WHOLE_TILE,
        },
        upload_queue::UploadQueue,
        view_state::ViewState,
    },
    style::layer::LayerPaint,
};

/// GPU metadata shared by background and atmosphere draws.
pub struct BackgroundBuffers {
    /// Background layer paint metadata.
    pub metadata_buffer: wgpu::Buffer,
    /// Dynamic uniform bind group supplying [`GlobeBackgroundUniform`] at `@group(1)`.
    pub globe_layer_bind_group: wgpu::BindGroup,
    /// Byte stride between entries in [`Self::globe_layer_bind_group`].
    pub globe_layer_stride: u32,
    /// Instance of each background style layer in `metadata_buffer`.
    pub instances: rustc_hash::FxHashMap<crate::style::StyleLayerId, u32>,
    /// The tile transform of each of `globe_tiles`, in order, used by globe meshes.
    pub tile_metadata_buffer: wgpu::Buffer,
    /// The tiles the globe's background is drawn on.
    pub globe_tiles: Vec<WorldTileCoords>,
    /// Evaluated atmosphere opacity.
    pub atmosphere_metadata_buffer: wgpu::Buffer,
    /// Sky colours and horizon, when the style has a sky the frame draws.
    pub sky_metadata_buffer: Option<wgpu::Buffer>,
    /// Whether an unclipped flat background fills the entire viewport cleanly.
    pub flat_covers_viewport: bool,
    /// Solid-color background layers whose evaluated alpha is `1.0` (`blend: None` eligible).
    pub opaque_layers: rustc_hash::FxHashSet<crate::style::StyleLayerId>,
    /// Generation matching [`BackgroundGpuBuffers`] buffer allocations; skips handle clones when unchanged.
    pub(crate) buffer_generation: u64,
}

/// Persistent GPU buffers updated in place via `UploadQueue::write_buffer`.
///
/// Pre-allocates GPU buffers with `COPY_DST` and grows capacity geometrically
/// whenever layer or visible globe tile counts exceed capacity, eliminating
/// per-frame GPU buffer creation.
#[derive(Default)]
pub struct BackgroundGpuBuffers {
    pub(super) inner: Option<AllocatedBuffers>,
    scratch: BackgroundScratch,
}

pub(super) struct AllocatedBuffers {
    pub(super) metadata_buffer: wgpu::Buffer,
    pub(super) metadata_capacity: usize,
    pub(super) last_metadata_bytes: Vec<u8>,
    pub(super) globe_uniform: GlobeLayerUniformPool,
    pub(super) tile_metadata_buffer: wgpu::Buffer,
    pub(super) tile_metadata_capacity: usize,
    pub(super) last_tile_metadata_bytes: Vec<u8>,
    atmosphere_metadata_buffer: wgpu::Buffer,
    last_atmosphere_bytes: Option<[u8; std::mem::size_of::<AtmosphereLayerMetadata>()]>,
    sky_metadata_buffer: wgpu::Buffer,
    last_sky_bytes: Option<[u8; std::mem::size_of::<SkyLayerMetadata>()]>,
    pub(super) buffer_generation: u64,
}

impl Drop for AllocatedBuffers {
    fn drop(&mut self) {
        self.metadata_buffer.destroy();
        self.tile_metadata_buffer.destroy();
        self.atmosphere_metadata_buffer.destroy();
        self.sky_metadata_buffer.destroy();
    }
}

const INITIAL_METADATA_CAPACITY: usize = 16;
const INITIAL_TILE_METADATA_CAPACITY: usize = 64;

impl AllocatedBuffers {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let metadata_capacity = INITIAL_METADATA_CAPACITY;
        let globe_uniform = GlobeLayerUniformPool::new(device, metadata_capacity);
        let metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Background Metadata Buffer"),
            size: (metadata_capacity * std::mem::size_of::<BackgroundLayerMetadata>())
                as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tile_metadata_capacity = INITIAL_TILE_METADATA_CAPACITY;
        let tile_metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Globe Background Tile Metadata Buffer"),
            size: (tile_metadata_capacity * std::mem::size_of::<ShaderTileMetadata>())
                as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let atmosphere_metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atmosphere Metadata Buffer"),
            size: std::mem::size_of::<AtmosphereLayerMetadata>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });

        let sky_metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sky Metadata Buffer"),
            size: std::mem::size_of::<SkyLayerMetadata>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            metadata_buffer,
            metadata_capacity,
            last_metadata_bytes: Vec::new(),
            globe_uniform,
            tile_metadata_buffer,
            tile_metadata_capacity,
            last_tile_metadata_bytes: Vec::new(),
            atmosphere_metadata_buffer,
            last_atmosphere_bytes: None,
            sky_metadata_buffer,
            last_sky_bytes: None,
            buffer_generation: 1,
        }
    }
}

impl AllocatedBuffers {
    pub(super) fn bump_generation(&mut self) {
        self.buffer_generation = self.buffer_generation.wrapping_add(1).max(1);
    }
}

fn write_slice_if_changed(
    queue: &UploadQueue,
    buffer: &wgpu::Buffer,
    bytes: &[u8],
    cached: &mut Vec<u8>,
) {
    if cached.as_slice() != bytes {
        queue.write_buffer(buffer, 0, bytes);
        cached.clear();
        cached.extend_from_slice(bytes);
    }
}

fn write_pod_if_changed<const N: usize>(
    queue: &UploadQueue,
    buffer: &wgpu::Buffer,
    bytes: &[u8],
    cached: &mut Option<[u8; N]>,
) {
    if cached.as_ref().is_none_or(|prev| prev.as_slice() != bytes) {
        queue.write_buffer(buffer, 0, bytes);
        *cached = bytes.try_into().ok();
    }
}

pub(super) fn write_buffer_slice<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &UploadQueue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut usize,
    last_bytes: &mut Vec<u8>,
    label: &str,
    items: &[T],
) -> bool {
    let needed_capacity = items.len().max(1);
    let reallocated = needed_capacity > *capacity;
    if reallocated {
        let new_capacity = (*capacity * 2).max(needed_capacity);
        queue.cancel_staged_buffer(buffer);
        buffer.destroy();
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (new_capacity * std::mem::size_of::<T>()) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *capacity = new_capacity;
        last_bytes.clear();
    }
    if !items.is_empty() {
        write_slice_if_changed(queue, buffer, bytemuck::cast_slice(items), last_bytes);
    }
    reallocated
}

impl BackgroundGpuBuffers {
    /// Initial capacity for background layer metadata (16 layers).
    pub const INITIAL_METADATA_CAPACITY: usize = 16;
    /// Initial capacity for globe tile metadata (64 tiles).
    pub const INITIAL_TILE_METADATA_CAPACITY: usize = 64;

    /// Creates persistent GPU buffers on `device` with initial capacity.
    pub fn new(device: &wgpu::Device) -> Self {
        let mut buffers = Self::default();
        buffers.ensure_allocated(device);
        buffers
    }

    fn ensure_allocated(&mut self, device: &wgpu::Device) -> &mut AllocatedBuffers {
        self.inner
            .get_or_insert_with(|| AllocatedBuffers::new(device))
    }

    /// Writes `metadatas` into `metadata_buffer`, reallocating with geometric growth if capacity is exceeded.
    pub fn write_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        metadatas: &[BackgroundLayerMetadata],
    ) -> &wgpu::Buffer {
        let inner = self.ensure_allocated(device);
        let slice_realloc = write_buffer_slice(
            device,
            queue,
            &mut inner.metadata_buffer,
            &mut inner.metadata_capacity,
            &mut inner.last_metadata_bytes,
            "Background Metadata Buffer",
            metadatas,
        );
        let uniform_realloc = inner.globe_uniform.write_layers(device, queue, metadatas);
        if slice_realloc || uniform_realloc {
            inner.bump_generation();
        }
        &inner.metadata_buffer
    }

    /// Writes `tile_metadata` into `tile_metadata_buffer`, reallocating with geometric growth if capacity is exceeded.
    pub fn write_tile_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        tile_metadata: &[ShaderTileMetadata],
    ) -> &wgpu::Buffer {
        let inner = self.ensure_allocated(device);
        if write_buffer_slice(
            device,
            queue,
            &mut inner.tile_metadata_buffer,
            &mut inner.tile_metadata_capacity,
            &mut inner.last_tile_metadata_bytes,
            "Globe Background Tile Metadata Buffer",
            tile_metadata,
        ) {
            inner.bump_generation();
        }
        &inner.tile_metadata_buffer
    }

    /// Writes `metadata` into `atmosphere_metadata_buffer`.
    pub fn write_atmosphere(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        metadata: &AtmosphereLayerMetadata,
    ) -> &wgpu::Buffer {
        let inner = self.ensure_allocated(device);
        write_pod_if_changed(
            queue,
            &inner.atmosphere_metadata_buffer,
            bytemuck::bytes_of(metadata),
            &mut inner.last_atmosphere_bytes,
        );
        &inner.atmosphere_metadata_buffer
    }

    /// Writes `sky` into `sky_metadata_buffer`.
    pub fn write_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        sky: &SkyLayerMetadata,
    ) -> &wgpu::Buffer {
        let inner = self.ensure_allocated(device);
        write_pod_if_changed(
            queue,
            &inner.sky_metadata_buffer,
            bytemuck::bytes_of(sky),
            &mut inner.last_sky_bytes,
        );
        &inner.sky_metadata_buffer
    }

    /// Returns the persistent background metadata buffer, or `None` if unallocated.
    pub fn metadata_buffer(&self) -> Option<&wgpu::Buffer> {
        self.inner.as_ref().map(|inner| &inner.metadata_buffer)
    }

    /// Returns the persistent globe tile metadata buffer, or `None` if unallocated.
    pub fn tile_metadata_buffer(&self) -> Option<&wgpu::Buffer> {
        self.inner.as_ref().map(|inner| &inner.tile_metadata_buffer)
    }

    /// Returns the persistent atmosphere metadata buffer, or `None` if unallocated.
    pub fn atmosphere_metadata_buffer(&self) -> Option<&wgpu::Buffer> {
        self.inner
            .as_ref()
            .map(|inner| &inner.atmosphere_metadata_buffer)
    }

    /// Returns the persistent sky metadata buffer, or `None` if unallocated.
    pub fn sky_metadata_buffer(&self) -> Option<&wgpu::Buffer> {
        self.inner.as_ref().map(|inner| &inner.sky_metadata_buffer)
    }

    /// Returns the allocated capacity of the metadata buffer in elements.
    pub fn metadata_capacity(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.metadata_capacity)
    }

    /// Returns the allocated capacity of the globe tile metadata buffer in elements.
    pub fn tile_metadata_capacity(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.tile_metadata_capacity)
    }
}

pub use crate::render::eye_covering::globe_tiles;

/// Calculates tile metadata for globe background tiles.
pub fn globe_tile_metadata(view_state: &ViewState, coords: WorldTileCoords) -> ShaderTileMetadata {
    globe_tile_metadata_with_vp(view_state, &view_state.gpu_view_projection(), coords)
}

/// Calculates tile metadata for globe background tiles in pure-globe mode (`transition >= 1.0`),
/// where shaders ignore per-tile `transform` matrices.
pub(crate) fn globe_tile_metadata_pure_globe(
    view_state: &ViewState,
    coords: WorldTileCoords,
) -> ShaderTileMetadata {
    let scale = view_state.zoom().scale_to_zoom_level(coords.z) as f32;
    ShaderTileMetadata {
        transform: cgmath::Matrix4::<f32>::from_scale(1.0).into(),
        zoom_factor: scale,
        viewport_width: view_state.width() as f32,
        viewport_height: view_state.height() as f32,
        tile_mercator_coords: crate::projection::renderer_data::tile_mercator_coordinates(
            coords.into_tile(crate::style::source::TileAddressingScheme::XYZ),
        )
        .into(),
        clip_antimeridian: u32::from(u8::from(coords.z) == 0),
        line_width_scale: 1.0,
        line_units_per_pixel: 8.0 * scale,
        clip_region: WHOLE_TILE,
        _pad: [0; 2],
    }
}

/// Calculates tile metadata for globe background tiles using a precomputed `gpu_vp`.
pub fn globe_tile_metadata_with_vp(
    view_state: &ViewState,
    gpu_vp: &crate::render::camera::ViewProjection,
    coords: WorldTileCoords,
) -> ShaderTileMetadata {
    let scale = view_state.zoom().scale_to_zoom_level(coords.z) as f32;
    ShaderTileMetadata {
        transform: gpu_vp
            .to_model_view_projection(coords.transform_for_zoom(view_state.zoom()))
            .downcast()
            .into(),
        zoom_factor: scale,
        viewport_width: view_state.width() as f32,
        viewport_height: view_state.height() as f32,
        tile_mercator_coords: crate::projection::renderer_data::tile_mercator_coordinates(
            coords.into_tile(crate::style::source::TileAddressingScheme::XYZ),
        )
        .into(),
        clip_antimeridian: u32::from(u8::from(coords.z) == 0),
        line_width_scale: 1.0,
        line_units_per_pixel: 8.0 * scale,
        clip_region: WHOLE_TILE,
        _pad: [0; 2],
    }
}

/// The background colour at a zoom, with its opacity multiplied into premultiplied-free alpha.
pub fn background_color(layer: &crate::style::layer::StyleLayer, zoom: f64) -> [f32; 4] {
    let Some(LayerPaint::Background(paint)) = &layer.paint else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let mut color: [f32; 4] = paint
        .background_color
        .as_ref()
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .map_or([0.0, 0.0, 0.0, 1.0], |color| {
            cint::Alpha::<cint::EncodedSrgb<f32>>::from(color).into()
        });
    let opacity = paint
        .background_opacity
        .as_ref()
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .unwrap_or(1.0);
    color[3] *= opacity.clamp(0.0, 1.0);
    color
}

#[cfg(test)]
mod tests;

/// Same as [`world_rows`], reusing a precomputed camera view-projection matrix when available.
pub(crate) fn world_rows_with_vp(
    view_state: &ViewState,
    view_projection: &Matrix4<f64>,
    edges: bool,
) -> [[f32; 4]; 2] {
    const OPEN: [[f32; 4]; 2] = [[0.0, 0.0, 0.0, -1e30], [0.0, 0.0, 0.0, 1e30]];
    if !edges {
        return OPEN;
    }
    let center = view_state.camera().position();
    let relative =
        view_projection * Matrix4::from_translation(Vector3::new(center.x, center.y, 0.0));
    let plane = Matrix3::new(
        relative.x.x,
        relative.x.y,
        relative.x.w,
        relative.y.x,
        relative.y.y,
        relative.y.w,
        relative.w.x,
        relative.w.y,
        relative.w.w,
    );
    let Some(inverse) = plane.invert() else {
        return OPEN;
    };
    let world = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    [
        [
            inverse.x.y as f32,
            inverse.y.y as f32,
            inverse.z.y as f32,
            (-center.y) as f32,
        ],
        [
            inverse.x.z as f32,
            inverse.y.z as f32,
            inverse.z.z as f32,
            (world - center.y) as f32,
        ],
    ]
}

/// The rows that tell a pixel's map row for the northern and southern edges of the flat map,
/// or ones that cut nothing where the map has no such edges.
pub fn world_rows(view_state: &ViewState, edges: bool) -> [[f32; 4]; 2] {
    if !edges {
        return [[0.0, 0.0, 0.0, -1e30], [0.0, 0.0, 0.0, 1e30]];
    }
    let vp = view_state.view_projection();
    world_rows_with_vp(view_state, &vp.0, edges)
}

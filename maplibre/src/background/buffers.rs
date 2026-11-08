//! Persistent GPU buffers and metadata construction for background, sky, and atmosphere rendering.

use cgmath::{Matrix3, Matrix4, SquareMatrix, Vector3};

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
    /// Instance of each background style layer in `metadata_buffer`.
    pub instances: std::collections::HashMap<String, u32>,
    /// The tile transform of each of `globe_tiles`, in order, used by globe meshes.
    pub tile_metadata_buffer: wgpu::Buffer,
    /// The tiles the globe's background is drawn on.
    pub globe_tiles: Vec<WorldTileCoords>,
    /// Evaluated atmosphere opacity.
    pub atmosphere_metadata_buffer: wgpu::Buffer,
    /// Sky colours and horizon, when the style has a sky the frame draws.
    pub sky_metadata_buffer: Option<wgpu::Buffer>,
}

/// Persistent GPU buffers updated in place via `UploadQueue::write_buffer`.
///
/// Pre-allocates GPU buffers with `COPY_DST` and grows capacity geometrically
/// whenever layer or visible globe tile counts exceed capacity, eliminating
/// per-frame GPU buffer creation.
#[derive(Default)]
pub struct BackgroundGpuBuffers {
    inner: Option<AllocatedBuffers>,
}

struct AllocatedBuffers {
    metadata_buffer: wgpu::Buffer,
    metadata_capacity: usize,
    tile_metadata_buffer: wgpu::Buffer,
    tile_metadata_capacity: usize,
    atmosphere_metadata_buffer: wgpu::Buffer,
    sky_metadata_buffer: wgpu::Buffer,
}

impl AllocatedBuffers {
    fn new(device: &wgpu::Device) -> Self {
        let metadata_capacity = BackgroundGpuBuffers::INITIAL_METADATA_CAPACITY;
        let metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Background Metadata Buffer"),
            size: (metadata_capacity * std::mem::size_of::<BackgroundLayerMetadata>())
                as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tile_metadata_capacity = BackgroundGpuBuffers::INITIAL_TILE_METADATA_CAPACITY;
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
            tile_metadata_buffer,
            tile_metadata_capacity,
            atmosphere_metadata_buffer,
            sky_metadata_buffer,
        }
    }
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
        if self.inner.is_none() {
            self.inner = Some(AllocatedBuffers::new(device));
        }
        self.inner.as_mut().expect("allocated buffers")
    }

    /// Writes `metadatas` into `metadata_buffer`, reallocating with geometric growth if capacity is exceeded.
    pub fn write_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        metadatas: &[BackgroundLayerMetadata],
    ) {
        let inner = self.ensure_allocated(device);
        let needed_capacity = metadatas.len().max(1);
        if needed_capacity > inner.metadata_capacity {
            let new_capacity = (inner.metadata_capacity * 2).max(needed_capacity);
            inner.metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Background Metadata Buffer"),
                size: (new_capacity * std::mem::size_of::<BackgroundLayerMetadata>())
                    as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            inner.metadata_capacity = new_capacity;
        }
        if !metadatas.is_empty() {
            queue.write_buffer(&inner.metadata_buffer, 0, bytemuck::cast_slice(metadatas));
        }
    }

    /// Writes `tile_metadata` into `tile_metadata_buffer`, reallocating with geometric growth if capacity is exceeded.
    pub fn write_tile_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        tile_metadata: &[ShaderTileMetadata],
    ) {
        let inner = self.ensure_allocated(device);
        let needed_capacity = tile_metadata.len().max(1);
        if needed_capacity > inner.tile_metadata_capacity {
            let new_capacity = (inner.tile_metadata_capacity * 2).max(needed_capacity);
            inner.tile_metadata_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Globe Background Tile Metadata Buffer"),
                size: (new_capacity * std::mem::size_of::<ShaderTileMetadata>())
                    as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            inner.tile_metadata_capacity = new_capacity;
        }
        if !tile_metadata.is_empty() {
            queue.write_buffer(
                &inner.tile_metadata_buffer,
                0,
                bytemuck::cast_slice(tile_metadata),
            );
        }
    }

    /// Writes `metadata` into `atmosphere_metadata_buffer`.
    pub fn write_atmosphere(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        metadata: &AtmosphereLayerMetadata,
    ) {
        let inner = self.ensure_allocated(device);
        queue.write_buffer(
            &inner.atmosphere_metadata_buffer,
            0,
            bytemuck::bytes_of(metadata),
        );
    }

    /// Writes `sky` into `sky_metadata_buffer`.
    pub fn write_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        sky: &SkyLayerMetadata,
    ) {
        let inner = self.ensure_allocated(device);
        queue.write_buffer(&inner.sky_metadata_buffer, 0, bytemuck::bytes_of(sky));
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

/// The tiles the globe's background is drawn on: the frame's covering, so that the ground
/// near an eye standing on the globe follows the sphere as closely as the tiles drawn there;
/// the world tile while no covering is selected.
pub fn globe_tiles(world: &crate::tcs::world::World) -> Vec<WorldTileCoords> {
    let covering: Vec<_> = crate::render::eye_covering::drawn_tiles(world)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|coords| coords.wrapped().is_some())
        .collect();
    if covering.is_empty() {
        vec![WorldTileCoords::default()]
    } else {
        covering
    }
}

/// Calculates tile metadata for globe background tiles.
pub fn globe_tile_metadata(view_state: &ViewState, coords: WorldTileCoords) -> ShaderTileMetadata {
    let scale = view_state.zoom().scale_to_tile(&coords) as f32;
    ShaderTileMetadata {
        transform: view_state
            .gpu_view_projection()
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

/// The sky draw's values for the frame: colours at the zoom and the horizon line on screen,
/// scaled to the physical frame height that `@builtin(position)` uses in the fragment shader.
pub fn sky_metadata(
    sky: &crate::style::sky::SkySpecification,
    view_state: &ViewState,
    projection_transition: f32,
    frame_height: f32,
) -> SkyLayerMetadata {
    let zoom = view_state.zoom().value();
    let colors = sky.colors_at(zoom);
    let logical_height = view_state.height();
    let scale = if logical_height > 0.0 && frame_height > 0.0 {
        f64::from(frame_height) / logical_height
    } else {
        1.0
    };
    let height = logical_height * scale;
    let mut horizon = view_state.horizon_line();
    horizon.point.x *= scale;
    horizon.point.y *= scale;
    let sky_blend = if view_state.opaque_environment() {
        0.0
    } else {
        projection_transition
    };
    let (inverse_projection, up_and_horizon) = if sky_blend > 0.0 {
        crate::render::projection::globe_camera_for_view(view_state)
            .ok()
            .and_then(|camera| {
                let mut inv: [[f32; 4]; 4] = camera.inverse_projection().cast::<f32>()?.into();
                let globe_pos = camera.globe_center_in_view().cast::<f32>()?;
                inv[2] = [
                    camera.camera_to_center_distance() as f32,
                    scale as f32,
                    camera.globe_radius_pixels() as f32,
                    sky.atmosphere_blend_at_zoom(zoom),
                ];
                Some((inv, [globe_pos.x, globe_pos.y, globe_pos.z, 0.0]))
            })
            .unwrap_or_else(|| {
                (
                    cgmath::Matrix4::from_scale(1.0).into(),
                    [0.0, 0.0, 1.0, 0.0],
                )
            })
    } else {
        (
            cgmath::Matrix4::from_scale(1.0).into(),
            [0.0, 0.0, 1.0, 0.0],
        )
    };
    SkyLayerMetadata {
        sky_color: colors.sky,
        horizon_color: colors.horizon,
        horizon: horizon.to_shader(),
        blend: [
            (f64::from(colors.sky_horizon_blend) * height / 2.0) as f32,
            sky_blend,
            height as f32,
            if view_state.has_external_view() {
                1.0e-8
            } else {
                0.0
            },
        ],
        inverse_projection,
        up_and_horizon,
    }
}

/// The rows that tell a pixel's map row for the northern and southern edges of the flat map,
/// or ones that cut nothing where the map has no such edges.
pub fn world_rows(view_state: &ViewState, edges: bool) -> [[f32; 4]; 2] {
    const OPEN: [[f32; 4]; 2] = [[0.0, 0.0, 0.0, -1e30], [0.0, 0.0, 0.0, 1e30]];
    if !edges {
        return OPEN;
    }
    let center = view_state.camera().position();
    let relative = view_state.view_projection().0
        * Matrix4::from_translation(Vector3::new(center.x, center.y, 0.0));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persistent_buffers_initialize_and_upload() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("device");
        let queue = UploadQueue::new(queue);

        let mut gpu_buffers = BackgroundGpuBuffers::new(&device);
        assert_eq!(
            gpu_buffers.metadata_capacity(),
            BackgroundGpuBuffers::INITIAL_METADATA_CAPACITY
        );
        assert_eq!(
            gpu_buffers.tile_metadata_capacity(),
            BackgroundGpuBuffers::INITIAL_TILE_METADATA_CAPACITY
        );

        let meta = BackgroundLayerMetadata {
            color: [1.0, 0.0, 0.0, 1.0],
            z_index: 0.0,
            padding: [0.0; 3],
            horizon: [0.0; 4],
            viewport: [600.0, 0.0, 800.0, 600.0],
            world_rows: [[0.0; 4]; 2],
        };
        gpu_buffers.write_metadata(&device, &queue, &[meta]);
        assert!(queue.take_written_bytes() > 0);

        let items: Vec<BackgroundLayerMetadata> = vec![meta; 32];
        gpu_buffers.write_metadata(&device, &queue, &items);
        assert_eq!(gpu_buffers.metadata_capacity(), 32);

        let items_64: Vec<BackgroundLayerMetadata> = vec![meta; 33];
        gpu_buffers.write_metadata(&device, &queue, &items_64);
        assert_eq!(gpu_buffers.metadata_capacity(), 64);
    }

    #[test]
    fn opacity_scales_alpha() {
        let layer = serde_json::json!({"id": "b", "type": "background", "paint": {"background-color": "blue", "background-opacity": 0.5}});
        let layer: crate::style::layer::StyleLayer =
            serde_json::from_value(layer).expect("valid background layer");
        let color = background_color(&layer, 0.0);
        assert_eq!(color, [0.0, 0.0, 1.0, 0.5]);
    }

    #[test]
    fn colour_follows_zoom() {
        let paint = serde_json::json!({"background-color": {
            "stops": [[0, "red"], [10, "blue"]], "base": 1.0
        }});
        let layer = serde_json::json!({"id": "b", "type": "background", "paint": paint});
        let layer: crate::style::layer::StyleLayer =
            serde_json::from_value(layer).expect("valid background layer");
        let start = background_color(&layer, 0.0);
        let end = background_color(&layer, 10.0);
        assert_eq!(start[..3], [1.0, 0.0, 0.0]);
        assert_eq!(end[..3], [0.0, 0.0, 1.0]);
    }
}

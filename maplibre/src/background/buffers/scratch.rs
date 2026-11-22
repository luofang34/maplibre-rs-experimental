use cgmath::{Matrix3, Matrix4, SquareMatrix, Vector3};

use super::{write_buffer_slice, AllocatedBuffers, BackgroundGpuBuffers};
use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    render::{
        shaders::{BackgroundLayerMetadata, ShaderTileMetadata},
        upload_queue::UploadQueue,
        view_state::ViewState,
    },
};

/// Persistent CPU staging containers reused across frames by `background::queue_system` so
/// steady-state frames avoid per-frame `Vec` and `FxHashMap`/`FxHashSet` heap allocations.
#[derive(Default)]
pub(crate) struct BackgroundScratch {
    pub(crate) metadatas: Vec<BackgroundLayerMetadata>,
    pub(crate) tile_metadata: Vec<ShaderTileMetadata>,
    pub(crate) globe_tiles: Vec<WorldTileCoords>,
    pub(crate) instances: rustc_hash::FxHashMap<crate::style::StyleLayerId, u32>,
    pub(crate) opaque_layers: rustc_hash::FxHashSet<crate::style::StyleLayerId>,
}

impl BackgroundGpuBuffers {
    /// Uploads `self.scratch.metadatas` into `metadata_buffer`, growing GPU capacity if needed.
    pub(crate) fn write_staged_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> &wgpu::Buffer {
        let Self { inner, scratch } = self;
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        write_buffer_slice(
            device,
            queue,
            &mut inner.metadata_buffer,
            &mut inner.metadata_capacity,
            &mut inner.last_metadata_bytes,
            "Background Metadata Buffer",
            &scratch.metadatas,
        );
        &inner.metadata_buffer
    }

    /// Uploads `self.scratch.tile_metadata` into `tile_metadata_buffer`, growing GPU capacity if
    /// needed.
    pub(crate) fn write_staged_tile_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> &wgpu::Buffer {
        let Self { inner, scratch } = self;
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        write_buffer_slice(
            device,
            queue,
            &mut inner.tile_metadata_buffer,
            &mut inner.tile_metadata_capacity,
            &mut inner.last_tile_metadata_bytes,
            "Globe Background Tile Metadata Buffer",
            &scratch.tile_metadata,
        );
        &inner.tile_metadata_buffer
    }
}

/// Populates `out` with the wrapped covering tiles used for the globe background, reusing `out`'s
/// existing heap allocation.
pub(crate) fn fill_globe_tiles(world: &crate::tcs::world::World, out: &mut Vec<WorldTileCoords>) {
    out.clear();
    if let Some(drawn) = crate::render::eye_covering::drawn_tiles(world) {
        out.extend(
            drawn
                .iter()
                .copied()
                .filter(|coords| coords.wrapped().is_some()),
        );
    }
    if out.is_empty() {
        out.push(WorldTileCoords::default());
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

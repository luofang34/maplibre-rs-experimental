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
    cached_bg_style_rev: Option<u64>,
    bg_layer_indices: Vec<usize>,
    last_pure_globe_params: Option<(u32, u32, u64)>,
    last_pure_globe_tiles: Vec<WorldTileCoords>,
}

impl BackgroundScratch {
    pub(crate) fn bg_layer_indices(&mut self, style: &crate::style::Style) -> &[usize] {
        let rev = style.revision();
        if self.cached_bg_style_rev != Some(rev) {
            self.bg_layer_indices.clear();
            for (idx, layer) in style.layers.iter().enumerate() {
                if layer.type_ == "background" {
                    self.bg_layer_indices.push(idx);
                }
            }
            self.cached_bg_style_rev = Some(rev);
        }
        &self.bg_layer_indices
    }

    pub(crate) fn pure_globe_tile_metadata_matches(
        &self,
        view_state: &ViewState,
        globe_tiles: &[WorldTileCoords],
    ) -> bool {
        let params = (
            (view_state.width() as f32).to_bits(),
            (view_state.height() as f32).to_bits(),
            view_state.zoom().value().to_bits(),
        );
        !self.tile_metadata.is_empty()
            && self.last_pure_globe_params == Some(params)
            && self.last_pure_globe_tiles.as_slice() == globe_tiles
    }

    pub(crate) fn record_pure_globe_tiles(
        &mut self,
        view_state: &ViewState,
        globe_tiles: &[WorldTileCoords],
    ) {
        self.last_pure_globe_params = Some((
            (view_state.width() as f32).to_bits(),
            (view_state.height() as f32).to_bits(),
            view_state.zoom().value().to_bits(),
        ));
        self.last_pure_globe_tiles.clear();
        self.last_pure_globe_tiles.extend_from_slice(globe_tiles);
    }

    pub(crate) fn invalidate_pure_globe_tiles(&mut self) {
        self.last_pure_globe_params = None;
    }
}

impl BackgroundGpuBuffers {
    /// Returns the buffer allocation generation (`0` until buffers are first allocated).
    #[inline]
    pub(crate) fn allocation_generation(&self) -> u64 {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.buffer_generation)
    }

    /// Uploads `self.scratch.metadatas` into `metadata_buffer`, growing GPU capacity if needed.
    pub(crate) fn write_staged_metadata(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> &wgpu::Buffer {
        let Self { inner, scratch } = self;
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        if write_buffer_slice(
            device,
            queue,
            &mut inner.metadata_buffer,
            &mut inner.metadata_capacity,
            &mut inner.last_metadata_bytes,
            "Background Metadata Buffer",
            &scratch.metadatas,
        ) {
            inner.bump_generation();
        }
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
        if write_buffer_slice(
            device,
            queue,
            &mut inner.tile_metadata_buffer,
            &mut inner.tile_metadata_capacity,
            &mut inner.last_tile_metadata_bytes,
            "Globe Background Tile Metadata Buffer",
            &scratch.tile_metadata,
        ) {
            inner.bump_generation();
        }
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

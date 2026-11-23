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
    last_globe_drawn: Vec<WorldTileCoords>,
    globe_tiles_populated: bool,
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

    /// Rebuilds `self.globe_tiles` only when `drawn` differs from the previous globe covering.
    pub(crate) fn sync_globe_tiles(&mut self, drawn: Option<&[WorldTileCoords]>) -> bool {
        let slice = drawn.unwrap_or(&[]);
        if self.globe_tiles_populated && self.last_globe_drawn.as_slice() == slice {
            return false;
        }
        self.last_globe_drawn.clear();
        self.last_globe_drawn.extend_from_slice(slice);
        self.globe_tiles.clear();
        self.globe_tiles.extend(
            slice
                .iter()
                .copied()
                .filter(|coords| coords.wrapped().is_some()),
        );
        if self.globe_tiles.is_empty() {
            self.globe_tiles.push(WorldTileCoords::default());
        } else {
            self.globe_tiles.sort_unstable_by_key(|coords| {
                (
                    crate::render::tile_mesh::globe_mesh_group_key(
                        *coords,
                        crate::render::tile_mesh::TileMeshUsage::Raster,
                    ),
                    coords.x,
                    coords.y,
                )
            });
        }
        self.globe_tiles_populated = true;
        true
    }

    pub(crate) fn clear_globe_tiles(&mut self) -> bool {
        let had = !self.globe_tiles.is_empty() || self.globe_tiles_populated;
        self.last_globe_drawn.clear();
        self.globe_tiles.clear();
        self.globe_tiles_populated = false;
        had
    }

    pub(crate) fn prepare_tile_metadata(
        &mut self,
        view_state: &ViewState,
        gpu_vp: Option<&crate::render::camera::ViewProjection>,
        pure_globe: bool,
        alloc_gen: u64,
    ) -> bool {
        if pure_globe {
            let params = (
                (view_state.width() as f32).to_bits(),
                (view_state.height() as f32).to_bits(),
                view_state.zoom().value().to_bits(),
            );
            if alloc_gen != 0
                && !self.tile_metadata.is_empty()
                && self.last_pure_globe_params == Some(params)
                && self.last_pure_globe_tiles.as_slice() == self.globe_tiles.as_slice()
            {
                return false;
            }
            self.tile_metadata.clear();
            self.tile_metadata.extend(
                self.globe_tiles
                    .iter()
                    .map(|coords| super::globe_tile_metadata_pure_globe(view_state, *coords)),
            );
            self.last_pure_globe_params = Some(params);
            self.last_pure_globe_tiles.clear();
            self.last_pure_globe_tiles
                .extend_from_slice(&self.globe_tiles);
            true
        } else {
            self.last_pure_globe_params = None;
            if self.globe_tiles.is_empty() {
                self.tile_metadata.clear();
                return alloc_gen == 0;
            }
            if let Some(vp) = gpu_vp {
                self.tile_metadata.clear();
                self.tile_metadata.extend(
                    self.globe_tiles
                        .iter()
                        .map(|coords| super::globe_tile_metadata_with_vp(view_state, vp, *coords)),
                );
            }
            true
        }
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
        let slice_realloc = write_buffer_slice(
            device,
            queue,
            &mut inner.metadata_buffer,
            &mut inner.metadata_capacity,
            &mut inner.last_metadata_bytes,
            "Background Metadata Buffer",
            &scratch.metadatas,
        );
        let uniform_realloc = inner
            .globe_uniform
            .write_layers(device, queue, &scratch.metadatas);
        if slice_realloc || uniform_realloc {
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

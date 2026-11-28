//! Frame-to-frame memoization for [`TileViewPattern::generate_pattern`] during sub-tile camera
//! motion where the visible covering, GPU pools, and loaded tile states did not change.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHasher};

mod key;

pub(crate) use key::{
    compute_fast_memo_key, compute_memo_key, FastPatternKey, PatternEpochSnapshot, PatternMemoKey,
};

use super::{clip_region, TileViewPattern};
use crate::{
    coords::{WorldTileCoords, Zoom},
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        camera::ViewProjection,
        resource::{vertex_buffer::formats::ShaderTileMetadata, Queue},
        tile_view_pattern::{SourceShapes, TileShape, ViewTile},
    },
};

/// Caches the most recent [`TileViewPattern`] key across frames so smooth panning or zooming
/// inside an unchanged covering only updates per-shape zoom transforms in place.
#[derive(Default)]
pub struct TileViewPatternMemo {
    key: Option<PatternMemoKey>,
    fast_key: Option<FastPatternKey>,
    last_zoom_bits: u64,
    scratch_view_tiles: Vec<ViewTile>,
}

impl TileViewPatternMemo {
    /// Checks the O(1) [`FastPatternKey`], updating zoom transforms on `pattern` if only zoom changed.
    pub(crate) fn try_reuse_fast<Q, B>(
        &mut self,
        pattern: &mut TileViewPattern<Q, B>,
        fast_key: Option<&FastPatternKey>,
        zoom: Zoom,
    ) -> bool {
        if pattern.is_empty() || !self.matches_fast_key(fast_key) {
            return false;
        }
        self.apply_zoom_if_changed(pattern, zoom);
        true
    }

    /// Checks the full [`PatternMemoKey`], updating `fast_key` and zoom transforms on hit.
    pub(crate) fn try_reuse_full<Q: crate::render::resource::Queue<B>, B>(
        &mut self,
        pattern: &mut TileViewPattern<Q, B>,
        next_key: Option<&PatternMemoKey>,
        fast_key: Option<FastPatternKey>,
        zoom: Zoom,
    ) -> bool {
        if pattern.is_empty() || !self.matches_key(next_key, pattern.drops_shapes()) {
            return false;
        }
        let zoom_changed = self.zoom_changed(zoom);
        self.record_hit(fast_key, zoom_changed.then_some(zoom));
        if zoom_changed {
            pattern.update_zoom_in_place(zoom);
        }
        true
    }

    fn apply_zoom_if_changed<Q, B>(&mut self, pattern: &mut TileViewPattern<Q, B>, zoom: Zoom) {
        if self.zoom_changed(zoom) {
            self.set_zoom(zoom);
            pattern.update_zoom_in_place(zoom);
        }
    }

    /// O(1) memo check using [`FastPatternKey`].
    fn matches_fast_key(&self, next_key: Option<&FastPatternKey>) -> bool {
        matches!(
            (&self.fast_key, next_key),
            (Some(cached), Some(next)) if cached.matches(next)
        )
    }

    fn record_hit(&mut self, fast_key: Option<FastPatternKey>, updated_zoom: Option<Zoom>) {
        self.fast_key = fast_key;
        if let Some(zoom) = updated_zoom {
            self.last_zoom_bits = zoom.value().to_bits();
        }
    }

    /// Whether the cached pattern still serves `next_key`. `order_matters` is set while the
    /// pattern dropped shapes past its buffer's end: which ones it dropped follows the region's
    /// order, so only then does a reordered region need a fresh pattern.
    #[cfg(test)]
    pub(crate) fn matches_key(
        &self,
        next_key: Option<&PatternMemoKey>,
        order_matters: bool,
    ) -> bool {
        matches!(
            (&self.key, next_key),
            (Some(cached), Some(next)) if cached.same_inputs(next, order_matters)
        )
    }

    #[cfg(not(test))]
    fn matches_key(&self, next_key: Option<&PatternMemoKey>, order_matters: bool) -> bool {
        matches!(
            (&self.key, next_key),
            (Some(cached), Some(next)) if cached.same_inputs(next, order_matters)
        )
    }

    fn zoom_changed(&self, zoom: Zoom) -> bool {
        self.last_zoom_bits != zoom.value().to_bits()
    }

    fn set_zoom(&mut self, zoom: Zoom) {
        self.last_zoom_bits = zoom.value().to_bits();
    }

    #[cfg(test)]
    pub(crate) fn record_key(&mut self, key: Option<PatternMemoKey>, zoom: Zoom) {
        self.key = key;
        self.last_zoom_bits = zoom.value().to_bits();
    }

    pub(crate) fn begin_rebuild(&mut self) -> Vec<ViewTile> {
        self.key = None;
        self.fast_key = None;
        self.scratch_view_tiles.clear();
        std::mem::take(&mut self.scratch_view_tiles)
    }

    pub(crate) fn finish_rebuild(
        &mut self,
        mut scratch: Vec<ViewTile>,
        key: Option<PatternMemoKey>,
        fast_key: Option<FastPatternKey>,
        zoom: Zoom,
    ) {
        scratch.clear();
        self.scratch_view_tiles = scratch;
        self.key = key;
        self.fast_key = fast_key;
        self.last_zoom_bits = zoom.value().to_bits();
    }

    pub(crate) fn put_scratch(&mut self, mut scratch: Vec<ViewTile>) {
        scratch.clear();
        self.scratch_view_tiles = scratch;
    }
}

impl<Q, B> TileViewPattern<Q, B> {
    pub(crate) fn is_empty(&self) -> bool {
        self.view_tiles.is_empty()
    }

    pub(crate) fn update_zoom_in_place(&mut self, zoom: Zoom) {
        for view_tile in &mut self.view_tiles {
            view_tile
                .vector
                .for_each_mut(&mut |shape| shape.set_zoom(zoom));
            for (_, shapes) in &mut view_tile.raster {
                shapes.for_each_mut(&mut |shape| shape.set_zoom(zoom));
            }
        }
        self.zoom_dirty = true;
    }
}

pub(super) fn view_proj_bits(vp: &ViewProjection) -> [u64; 16] {
    let cols: &[[f64; 4]; 4] = vp.0.as_ref();
    let mut out = [0_u64; 16];
    let mut idx = 0;
    for col in cols {
        for &val in col {
            if let Some(slot) = out.get_mut(idx) {
                *slot = val.to_bits();
            }
            idx += 1;
        }
    }
    out
}

fn refresh_shapes(
    shapes: &SourceShapes,
    scratch: &mut [ShaderTileMetadata],
    view_proj: &ViewProjection,
    viewport: (f32, f32),
    style_zoom: Zoom,
    flags: (bool, bool, bool, bool, bool),
) {
    let (pure_globe, needs_transform, zoom_changed, viewport_changed, style_zoom_changed) = flags;
    shapes.for_each(&mut |shape: &TileShape| {
        let Some(idx) = shape.buffer_range().and_then(|r| {
            usize::try_from(r.start / std::mem::size_of::<ShaderTileMetadata>() as u64).ok()
        }) else {
            return;
        };
        let Some(entry) = scratch.get_mut(idx) else {
            return;
        };
        if needs_transform {
            entry.transform = if pure_globe {
                cgmath::Matrix4::<f32>::from_scale(1.0).into()
            } else {
                view_proj
                    .to_model_view_projection(shape.transform)
                    .downcast()
                    .into()
            };
        }
        if zoom_changed {
            entry.zoom_factor = shape.zoom_factor as f32;
        }
        if viewport_changed {
            entry.viewport_width = viewport.0;
            entry.viewport_height = viewport.1;
        }
        if style_zoom_changed {
            entry.line_units_per_pixel =
                8.0 * style_zoom.scale_to_zoom_level(shape.coords().z) as f32;
        }
    });
}

type PatternUploadParams = (u64, u32, u32, u64, bool);

impl<Q: Queue<B>, B> TileViewPattern<Q, B> {
    pub(super) fn refresh_cached_upload(
        &mut self,
        queue: &Q,
        view_proj: &ViewProjection,
        viewport: (f32, f32),
        style_zoom: Zoom,
        (pure_globe, params, vp_bits): (bool, PatternUploadParams, Option<[u64; 16]>),
    ) {
        let vp_changed = !pure_globe
            && (self.last_vp_bits != vp_bits || self.last_upload_params.is_some_and(|p| p.4));
        let zoom_changed = self.zoom_dirty;
        let viewport_changed = self
            .last_upload_params
            .is_some_and(|p| p.1 != params.1 || p.2 != params.2);
        let style_zoom_changed = self.last_upload_params.is_some_and(|p| p.3 != params.3);
        let needs_transform = vp_changed || (!pure_globe && zoom_changed);
        if needs_transform || zoom_changed || viewport_changed || style_zoom_changed {
            for view_tile in &self.view_tiles {
                refresh_shapes(
                    &view_tile.vector,
                    &mut self.metadata_scratch,
                    view_proj,
                    viewport,
                    style_zoom,
                    (
                        pure_globe,
                        needs_transform,
                        zoom_changed,
                        viewport_changed,
                        style_zoom_changed,
                    ),
                );
                for (_, shapes) in &view_tile.raster {
                    refresh_shapes(
                        shapes,
                        &mut self.metadata_scratch,
                        view_proj,
                        viewport,
                        style_zoom,
                        (
                            pure_globe,
                            needs_transform,
                            zoom_changed,
                            viewport_changed,
                            style_zoom_changed,
                        ),
                    );
                }
            }
        }
        self.commit_metadata_upload(queue, (params, vp_bits));
    }

    pub(super) fn commit_metadata_upload(
        &mut self,
        queue: &Q,
        (mut params, vp_bits): (PatternUploadParams, Option<[u64; 16]>),
    ) {
        params.0 = self.topology_generation;
        self.uploaded = self.metadata_scratch.len() as u64;
        let raw_buffer = bytemuck::cast_slice(self.metadata_scratch.as_slice());
        if self.last_uploaded_bytes.as_slice() != raw_buffer {
            queue.write_buffer(&self.view_tiles_buffer.inner, 0, raw_buffer);
            self.last_uploaded_bytes.clear();
            self.last_uploaded_bytes.extend_from_slice(raw_buffer);
        }
        self.zoom_dirty = false;
        self.last_upload_params = Some(params);
        self.last_vp_bits = vp_bits;
    }

    pub(super) fn rebuild_pattern_upload(
        &mut self,
        queue: &Q,
        capacity: usize,
        view_proj: &ViewProjection,
        viewport: (f32, f32),
        style_zoom: Zoom,
        (pure_globe, params, vp_bits): (bool, PatternUploadParams, Option<[u64; 16]>),
    ) {
        let mut buffer = std::mem::take(&mut self.metadata_scratch);
        buffer.clear();
        let mut stencil_ids = std::mem::take(&mut self.stencil_ids);
        stencil_ids.clear();
        let mut upload = PatternUpload {
            capacity,
            buffer: &mut buffer,
            stencil_ids: &mut stencil_ids,
            next_stencil: 0,
            skipped: 0,
            view_proj,
            viewport,
            style_zoom,
            pure_globe,
        };
        let mut topo_hasher = FxHasher::default();
        self.view_tiles.len().hash(&mut topo_hasher);
        for view_tile in &mut self.view_tiles {
            upload.push_view_tile(view_tile, &mut topo_hasher);
        }
        let skipped = upload.skipped;
        self.skipped_shapes = skipped;
        self.record_topology_hash(topo_hasher.finish());
        if skipped > 0 {
            tracing::warn!(
                skipped,
                capacity,
                "tile pattern exceeds its buffer; distant shapes are skipped this frame"
            );
        }
        self.stencil_ids = stencil_ids;
        self.metadata_scratch = buffer;
        self.topology_dirty = false;
        self.last_capacity = capacity;
        self.commit_metadata_upload(queue, (params, vp_bits));
    }
}

struct PatternUpload<'a> {
    capacity: usize,
    buffer: &'a mut Vec<ShaderTileMetadata>,
    stencil_ids: &'a mut FxHashMap<(i32, WorldTileCoords), u8>,
    next_stencil: u8,
    skipped: usize,
    view_proj: &'a ViewProjection,
    viewport: (f32, f32),
    style_zoom: Zoom,
    pure_globe: bool,
}

impl PatternUpload<'_> {
    fn push_view_tile(&mut self, view_tile: &mut ViewTile, hasher: &mut FxHasher) {
        view_tile.target.hash(hasher);
        let mut v_count = 0usize;
        view_tile.vector.for_each_mut(&mut |shape| {
            self.push_shape(shape);
            v_count += 1;
            hash_shape_topology(shape, hasher);
        });
        v_count.hash(hasher);
        view_tile.raster.len().hash(hasher);
        for (source_id, shapes) in &mut view_tile.raster {
            source_id.hash(hasher);
            let mut r_count = 0usize;
            shapes.for_each_mut(&mut |shape| {
                self.push_shape(shape);
                r_count += 1;
                hash_shape_topology(shape, hasher);
            });
            r_count.hash(hasher);
        }
    }

    fn push_shape(&mut self, shape: &mut TileShape) {
        if self.buffer.len() >= self.capacity {
            // A pitched view falling back to many small child tiles can exceed the
            // buffer; shapes past the end stay unrendered until their own tiles load.
            shape.clear_buffer_range();
            self.skipped += 1;
            return;
        }
        let stencil_coords = shape.stencil_coords();
        let stencil_ref = *self
            .stencil_ids
            .entry((shape.wrap, stencil_coords))
            .or_insert_with(|| {
                if self.next_stencil < 124 {
                    let assigned = self.next_stencil;
                    self.next_stencil += 1;
                    assigned
                } else {
                    stencil_coords.stencil_reference_value_3d()
                }
            });
        shape.set_stencil_reference(stencil_ref);
        shape.set_buffer_range(self.buffer.len() as u64);
        let transform = if self.pure_globe {
            cgmath::Matrix4::<f32>::from_scale(1.0).into()
        } else {
            self.view_proj
                .to_model_view_projection(shape.transform)
                .downcast()
                .into()
        };
        let mut mercator = tile_mercator_coordinates(
            shape
                .coords()
                .into_tile(crate::style::source::TileAddressingScheme::XYZ),
        );
        // A copy of the world lies whole worlds over in Mercator units.
        mercator.x += shape.wrap as f32;
        self.buffer.push(ShaderTileMetadata {
            transform,
            zoom_factor: shape.zoom_factor as f32,
            viewport_width: self.viewport.0,
            viewport_height: self.viewport.1,
            tile_mercator_coords: mercator.into(),
            line_width_scale: 1.0,
            line_units_per_pixel: 8.0
                * self.style_zoom.scale_to_zoom_level(shape.coords().z) as f32,
            clip_antimeridian: u32::from(u8::from(shape.coords().z) == 0),
            clip_region: clip_region(shape),
            _pad: [0; 2],
        });
    }
}

fn hash_shape_topology(shape: &TileShape, hasher: &mut FxHasher) {
    shape.coords().hash(hasher);
    shape.wrap.hash(hasher);
    shape.is_drawn_within_another().hash(hasher);
    shape.stencil_reference().hash(hasher);
    shape.buffer_range().hash(hasher);
}

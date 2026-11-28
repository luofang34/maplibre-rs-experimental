use super::{write_buffer_slice, AllocatedBuffers, BackgroundGpuBuffers};
use crate::{
    coords::WorldTileCoords,
    render::{
        shaders::{
            AtmosphereLayerMetadata, BackgroundLayerMetadata, ShaderTileMetadata, SkyLayerMetadata,
        },
        upload_queue::UploadQueue,
        view_state::ViewState,
    },
};

/// Persistent CPU staging containers reused across frames by `background::queue_system` so
/// steady-state frames avoid per-frame `Vec` and `FxHashMap`/`FxHashSet` heap allocations.
#[derive(Default)]
pub(super) struct BackgroundScratch {
    metadatas: Vec<BackgroundLayerMetadata>,
    tile_metadata: Vec<ShaderTileMetadata>,
    pub(super) globe_tiles: Vec<WorldTileCoords>,
    instances: rustc_hash::FxHashMap<crate::style::StyleLayerId, u32>,
    opaque_layers: rustc_hash::FxHashSet<crate::style::StyleLayerId>,
    cached_bg_style_rev: Option<u64>,
    bg_layer_indices: Vec<usize>,
    last_pure_globe_params: Option<(u32, u32, u64)>,
    last_pure_globe_tiles: Vec<WorldTileCoords>,
    last_globe_drawn: Vec<WorldTileCoords>,
    incoming_globe_drawn: Vec<WorldTileCoords>,
    globe_tiles_populated: bool,
}

impl BackgroundScratch {
    fn reset_frame(&mut self) {
        self.metadatas.clear();
        self.instances.clear();
        self.opaque_layers.clear();
    }

    pub(super) fn begin_background_frame(&mut self, style: &crate::style::Style) -> usize {
        self.reset_frame();
        self.bg_layer_indices(style).len()
    }

    pub(super) fn style_background_layer<'s>(
        &mut self,
        style: &'s crate::style::Style,
        pos: usize,
    ) -> Option<&'s crate::style::layer::StyleLayer> {
        self.bg_layer_indices(style)
            .get(pos)
            .and_then(|&idx| style.layers.get(idx))
    }

    pub(super) fn record_background_layer(
        &mut self,
        layer_id: crate::style::StyleLayerId,
        metadata: BackgroundLayerMetadata,
        opaque: bool,
    ) {
        if opaque {
            self.opaque_layers.insert(layer_id);
        }
        self.instances.insert(layer_id, self.metadatas.len() as u32);
        self.metadatas.push(metadata);
    }

    pub(super) fn has_background_layer(&self, layer_id: crate::style::StyleLayerId) -> bool {
        self.instances.contains_key(&layer_id)
    }

    pub(super) fn has_drawn_layers(&self) -> bool {
        !self.metadatas.is_empty()
    }

    pub(super) fn ensure_default_metadata(&mut self, default_metadata: BackgroundLayerMetadata) {
        if self.metadatas.is_empty() {
            self.metadatas.push(default_metadata);
        }
    }

    pub(super) fn apply_published_state(
        &mut self,
        instances: &mut rustc_hash::FxHashMap<crate::style::StyleLayerId, u32>,
        opaque_layers: &mut rustc_hash::FxHashSet<crate::style::StyleLayerId>,
        globe_tiles: &mut Vec<WorldTileCoords>,
        globe_tiles_changed: bool,
    ) {
        std::mem::swap(instances, &mut self.instances);
        std::mem::swap(opaque_layers, &mut self.opaque_layers);
        if globe_tiles_changed || globe_tiles.len() != self.globe_tiles.len() {
            globe_tiles.clone_from(&self.globe_tiles);
        }
    }

    pub(super) fn build_background_buffers(
        &mut self,
        (metadata_buffer, globe_layer, tile_metadata_buffer): (
            wgpu::Buffer,
            (wgpu::BindGroup, u32),
            wgpu::Buffer,
        ),
        (atmosphere_metadata_buffer, sky_metadata_buffer): (wgpu::Buffer, Option<wgpu::Buffer>),
        flat_covers_viewport: bool,
        buf_gen: u64,
    ) -> super::BackgroundBuffers {
        super::BackgroundBuffers {
            metadata_buffer,
            globe_layer_bind_group: globe_layer.0,
            globe_layer_stride: globe_layer.1,
            instances: std::mem::take(&mut self.instances),
            tile_metadata_buffer,
            globe_tiles: self.globe_tiles.clone(),
            atmosphere_metadata_buffer,
            sky_metadata_buffer,
            flat_covers_viewport,
            opaque_layers: std::mem::take(&mut self.opaque_layers),
            buffer_generation: buf_gen,
        }
    }

    fn upload_metadata<'a>(
        &self,
        inner: &'a mut Option<AllocatedBuffers>,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> &'a wgpu::Buffer {
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        let slice_realloc = write_buffer_slice(
            device,
            queue,
            &mut inner.metadata_buffer,
            &mut inner.metadata_capacity,
            &mut inner.last_metadata_bytes,
            "Background Metadata Buffer",
            &self.metadatas,
        );
        let uniform_realloc = inner
            .globe_uniform
            .write_layers(device, queue, &self.metadatas);
        if slice_realloc || uniform_realloc {
            inner.bump_generation();
        }
        &inner.metadata_buffer
    }

    fn upload_tile_metadata<'a>(
        &self,
        inner: &'a mut Option<AllocatedBuffers>,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> &'a wgpu::Buffer {
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        if write_buffer_slice(
            device,
            queue,
            &mut inner.tile_metadata_buffer,
            &mut inner.tile_metadata_capacity,
            &mut inner.last_tile_metadata_bytes,
            "Globe Background Tile Metadata Buffer",
            &self.tile_metadata,
        ) {
            inner.bump_generation();
        }
        &inner.tile_metadata_buffer
    }

    fn upload_globe_layers(
        &self,
        inner: &mut Option<AllocatedBuffers>,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> (wgpu::BindGroup, u32) {
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        if inner
            .globe_uniform
            .write_layers(device, queue, &self.metadatas)
        {
            inner.bump_generation();
        }
        inner.globe_uniform.cloned_binding()
    }

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
        // A covering lists tiles nearest the focus first; compared sorted, an orbit that only
        // reorders it leaves the instance buffer alone.
        self.incoming_globe_drawn.clear();
        self.incoming_globe_drawn.extend_from_slice(slice);
        self.incoming_globe_drawn.sort_unstable();
        if self.globe_tiles_populated && self.last_globe_drawn == self.incoming_globe_drawn {
            return false;
        }
        std::mem::swap(&mut self.last_globe_drawn, &mut self.incoming_globe_drawn);
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

    /// Prepares per-frame background staging containers and returns the style background count.
    pub(crate) fn begin_background_frame(&mut self, style: &crate::style::Style) -> usize {
        self.scratch.begin_background_frame(style)
    }

    /// Returns the background style layer at `pos`, if present in the cached style index.
    pub(crate) fn style_background_layer<'s>(
        &mut self,
        style: &'s crate::style::Style,
        pos: usize,
    ) -> Option<&'s crate::style::layer::StyleLayer> {
        self.scratch.style_background_layer(style, pos)
    }

    /// Appends one staged background layer entry and records its instance slot and opacity flag.
    pub(crate) fn record_background_layer(
        &mut self,
        layer_id: crate::style::StyleLayerId,
        metadata: BackgroundLayerMetadata,
        opaque: bool,
    ) {
        self.scratch
            .record_background_layer(layer_id, metadata, opaque)
    }

    /// Reports whether `layer_id` was recorded into the current frame's staged layer map.
    pub(crate) fn has_background_layer(&self, layer_id: crate::style::StyleLayerId) -> bool {
        self.scratch.has_background_layer(layer_id)
    }

    /// Reports whether at least one background layer metadata entry was staged this frame.
    pub(crate) fn has_drawn_layers(&self) -> bool {
        self.scratch.has_drawn_layers()
    }

    /// Inserts `default_metadata` when no background layer metadata was staged for this frame.
    pub(crate) fn ensure_default_metadata(&mut self, default_metadata: BackgroundLayerMetadata) {
        self.scratch.ensure_default_metadata(default_metadata)
    }

    /// Builds staged globe/flat tile metadata matrices for the current frame.
    pub(crate) fn prepare_tile_metadata(
        &mut self,
        view_state: &ViewState,
        gpu_vp: Option<&crate::render::camera::ViewProjection>,
        pure_globe: bool,
        alloc_gen: u64,
    ) -> bool {
        self.scratch
            .prepare_tile_metadata(view_state, gpu_vp, pure_globe, alloc_gen)
    }

    /// Rebuilds staged globe tile instances when `drawn` differs from the previous globe covering.
    pub(crate) fn sync_globe_tiles(&mut self, drawn: Option<&[WorldTileCoords]>) -> bool {
        self.scratch.sync_globe_tiles(drawn)
    }

    /// Clears staged globe tile instances when leaving globe mode.
    pub(crate) fn clear_globe_tiles(&mut self) -> bool {
        self.scratch.clear_globe_tiles()
    }

    /// Moves staged CPU instance/opaque sets into published [`BackgroundBuffers`] state.
    pub(crate) fn apply_published_state(
        &mut self,
        instances: &mut rustc_hash::FxHashMap<crate::style::StyleLayerId, u32>,
        opaque_layers: &mut rustc_hash::FxHashSet<crate::style::StyleLayerId>,
        globe_tiles: &mut Vec<WorldTileCoords>,
        globe_tiles_changed: bool,
    ) {
        self.scratch.apply_published_state(
            instances,
            opaque_layers,
            globe_tiles,
            globe_tiles_changed,
        )
    }

    /// Uploads staged buffers and constructs a populated [`BackgroundBuffers`](super::BackgroundBuffers).
    pub(crate) fn build_background_buffers(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        flat_covers_viewport: bool,
        (sky_metadata, atmosphere_metadata): (
            Option<&SkyLayerMetadata>,
            Option<&AtmosphereLayerMetadata>,
        ),
    ) -> super::BackgroundBuffers {
        let Self { inner, scratch } = self;
        let metadata_buffer = scratch.upload_metadata(inner, device, queue).clone();
        let globe_layer = scratch.upload_globe_layers(inner, device, queue);
        let tile_metadata_buffer = scratch.upload_tile_metadata(inner, device, queue).clone();
        let disabled = AtmosphereLayerMetadata::disabled();
        let atmo_ref = atmosphere_metadata.unwrap_or(&disabled);
        let atmosphere_metadata_buffer = self.write_atmosphere(device, queue, atmo_ref).clone();
        let sky_metadata_buffer =
            sky_metadata.map(|sky| self.write_sky(device, queue, sky).clone());
        let buf_gen = self.allocation_generation();
        self.scratch.build_background_buffers(
            (metadata_buffer, globe_layer, tile_metadata_buffer),
            (atmosphere_metadata_buffer, sky_metadata_buffer),
            flat_covers_viewport,
            buf_gen,
        )
    }

    /// Uploads staged background layer metadata and optionally staged globe tile metadata.
    pub(crate) fn flush_staged(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        write_tiles: bool,
    ) {
        let Self { inner, scratch } = self;
        let _ = scratch.upload_metadata(inner, device, queue);
        if write_tiles {
            let _ = scratch.upload_tile_metadata(inner, device, queue);
        }
    }

    /// Synchronizes published GPU buffer handles onto `existing` when allocations changed or
    /// updates optional sky metadata visibility otherwise.
    pub(crate) fn refresh_published_buffers(
        &mut self,
        existing: &mut super::BackgroundBuffers,
        device: &wgpu::Device,
        queue: &UploadQueue,
        buf_gen: u64,
        (sky_metadata, _atmosphere_metadata): (
            Option<&SkyLayerMetadata>,
            Option<&AtmosphereLayerMetadata>,
        ),
    ) {
        if existing.buffer_generation != buf_gen {
            self.sync_allocated_published_buffers(existing, device, queue, buf_gen, sky_metadata);
        } else {
            self.sync_sky_metadata_buffer(existing, sky_metadata);
        }
    }

    fn sync_allocated_published_buffers(
        &mut self,
        existing: &mut super::BackgroundBuffers,
        device: &wgpu::Device,
        queue: &UploadQueue,
        buf_gen: u64,
        sky_metadata: Option<&SkyLayerMetadata>,
    ) {
        existing.metadata_buffer = self.metadata_buffer().cloned().unwrap_or_else(|| {
            let Self { inner, scratch } = self;
            scratch.upload_metadata(inner, device, queue).clone()
        });
        let globe_layer = self.globe_layer_binding().unwrap_or_else(|| {
            let Self { inner, scratch } = self;
            scratch.upload_globe_layers(inner, device, queue)
        });
        existing.globe_layer_bind_group = globe_layer.0;
        existing.globe_layer_stride = globe_layer.1;
        existing.tile_metadata_buffer = self.tile_metadata_buffer().cloned().unwrap_or_else(|| {
            let Self { inner, scratch } = self;
            scratch.upload_tile_metadata(inner, device, queue).clone()
        });
        existing.atmosphere_metadata_buffer = self
            .atmosphere_metadata_buffer()
            .cloned()
            .unwrap_or_else(|| {
                let disabled = AtmosphereLayerMetadata::disabled();
                self.write_atmosphere(device, queue, &disabled).clone()
            });
        existing.sky_metadata_buffer =
            sky_metadata.and_then(|_| self.sky_metadata_buffer().cloned());
        existing.buffer_generation = buf_gen;
    }

    fn sync_sky_metadata_buffer(
        &self,
        existing: &mut super::BackgroundBuffers,
        sky_metadata: Option<&SkyLayerMetadata>,
    ) {
        if sky_metadata.is_some() {
            if existing.sky_metadata_buffer.is_none() {
                existing.sky_metadata_buffer = self.sky_metadata_buffer().cloned();
            }
        } else {
            existing.sky_metadata_buffer = None;
        }
    }
}

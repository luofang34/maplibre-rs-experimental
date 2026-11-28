//! Collision-free lightweight handle identifiers and pass-scoped resource snapshots so
//! `Stage::Render` avoids per-draw `wgpu` handle clones and `World::resources` hash lookups.

use std::{
    any::Any,
    hash::{Hash, Hasher},
};

use smallvec::SmallVec;
use wgpu::{Adapter, Device, DownlevelFlags, Features, Limits};

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        tile_mesh::GlobeTileMeshCache,
        tile_view_pattern::WgpuTileViewPattern,
        MaskPipeline,
    },
    tcs::world::World,
};

/// Direct typed pool reference matching `VectorBufferPool` without erasing through `dyn Any`.
pub(crate) type VectorPoolRef = crate::render::resource::BufferPool<
    crate::render::upload_queue::UploadQueue,
    wgpu::Buffer,
    crate::render::ShaderVertex,
    crate::render::resource::IndexDataType,
    crate::render::shaders::ShaderLayerMetadata,
    crate::render::shaders::FillShaderFeatureMetadata,
>;

/// Direct typed pool reference matching `SymbolBufferPool` without erasing through `dyn Any`.
pub(crate) type SymbolPoolRef = crate::render::resource::BufferPool<
    crate::render::upload_queue::UploadQueue,
    wgpu::Buffer,
    crate::render::shaders::ShaderSymbolVertex,
    crate::render::resource::IndexDataType,
    crate::render::shaders::EmptyLayerMetadata,
    crate::render::shaders::SDFShaderFeatureMetadata,
>;

/// Typed pass-scoped slot indices populated by domain plugins once per render pass.
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DomainPassSlot {
    VectorPipeline = 0,
    CirclePipeline = 1,
    LinePipeline = 2,
    ExtrusionPipeline = 3,
    PatternResources = 4,
    LineDashes = 5,
    TerrainResources = 6,
    SymbolPipeline = 7,
    SymbolDepth = 8,
    SymbolCovering = 9,
    SymbolTextures = 10,
    RasterResources = 11,
    RasterCrossFade = 12,
}

const DOMAIN_SLOT_COUNT: usize = 13;

/// Callback populating one domain's slots into [`PassFrameRefs`] at render-pass start.
pub(crate) type PassRefPopulator = for<'w> fn(&'w World, &mut PassFrameRefs<'w>);

/// Registered domain populators invoked in [`PassFrameRefs::from_world_for`].
#[derive(Default)]
pub(crate) struct PassRefPopulators(Vec<(u8, PassRefPopulator)>);

impl PassRefPopulators {
    /// Registers `populator` idempotently so its domain slots populate relevant render passes.
    pub(crate) fn register(world: &mut World, domain: u8, populator: PassRefPopulator) {
        let populators = &mut world.resources.get_or_init_mut::<Self>().0;
        if let Some(entry) = populators
            .iter_mut()
            .find(|(_, existing)| std::ptr::fn_addr_eq(*existing, populator))
        {
            entry.0 |= domain;
        } else {
            populators.push((domain, populator));
        }
    }
}

/// What the adapter and device's draw calls can do beyond what WebGL2 guarantees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCapabilities {
    /// Whether an indexed draw may add a base vertex to its indices; WebGL2 cannot, so pooled
    /// geometry is then bound from its own offset instead of drawn from the whole buffer.
    pub base_vertex: bool,
    /// Whether indirect draw commands (`draw_indirect`, `draw_indexed_indirect`) are supported.
    pub indirect_execution: bool,
    /// Whether indirect draw commands may specify a non-zero `first_instance`.
    pub indirect_first_instance: bool,
    /// Whether multi-draw indirect submission (`multi_draw_indirect`, `multi_draw_indexed_indirect`) is enabled.
    pub multi_draw_indirect: bool,
    /// Whether GPU-counted multi-draw indirect (`multi_draw_indirect_count`, `multi_draw_indexed_indirect_count`) is enabled.
    pub multi_draw_indirect_count: bool,
    /// Whether vertex shaders can read storage buffers for per-instance pulling.
    pub vertex_storage_buffers: bool,
}

impl DrawCapabilities {
    /// The capabilities `adapter` reports before device feature/limit negotiation.
    pub fn of(adapter: &Adapter) -> Self {
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let features = adapter.features();
        let limits = adapter.limits();
        Self::from_raw(downlevel, features, &limits)
    }

    /// The capabilities negotiated between `adapter`'s downlevel flags and `device`'s enabled
    /// features and limits.
    pub fn from_adapter_and_device(adapter: &Adapter, device: &Device) -> Self {
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let features = device.features();
        let limits = device.limits();
        Self::from_raw(downlevel, features, &limits)
    }

    fn from_raw(downlevel: DownlevelFlags, features: Features, limits: &Limits) -> Self {
        let indirect_execution = downlevel.contains(DownlevelFlags::INDIRECT_EXECUTION);
        Self {
            base_vertex: downlevel.contains(DownlevelFlags::BASE_VERTEX),
            indirect_execution,
            indirect_first_instance: features.contains(Features::INDIRECT_FIRST_INSTANCE),
            multi_draw_indirect: indirect_execution,
            multi_draw_indirect_count: indirect_execution
                && features.contains(Features::MULTI_DRAW_INDIRECT_COUNT),
            vertex_storage_buffers: downlevel.contains(DownlevelFlags::VERTEX_STORAGE)
                && limits.max_storage_buffers_per_shader_stage > 0,
        }
    }

    /// Whether single-batch indirect draw arguments with non-zero `first_instance` (and
    /// `base_vertex` when `indexed` is `true`) are valid on this device.
    #[inline]
    pub const fn supports_indirect_batching(&self, indexed: bool) -> bool {
        self.indirect_execution && self.indirect_first_instance && (!indexed || self.base_vertex)
    }

    /// Whether cross-bin multi-draw indirect aggregation is valid for `indexed` draws.
    #[inline]
    pub const fn supports_multidraw_indirect(&self, indexed: bool) -> bool {
        self.multi_draw_indirect && self.supports_indirect_batching(indexed)
    }
}

#[inline]
pub(crate) fn initialized<T: 'static>(world: &World) -> Option<&T> {
    match world.resources.get::<Eventually<T>>() {
        Some(Initialized(value)) => Some(value),
        _ => None,
    }
}

/// Immutable render pipelines, pools, and frame uniforms resolved once per render pass.
#[derive(Clone, Copy, Default)]
pub(crate) struct PassFrameRefs<'w> {
    pub capabilities: Option<&'w DrawCapabilities>,
    pub projection: Option<&'w ProjectionGpuResources>,
    pub tile_view_pattern: Option<&'w WgpuTileViewPattern>,
    pub tile_mesh_cache: Option<&'w GlobeTileMeshCache>,
    pub mask_pipeline: Option<&'w MaskPipeline>,
    pub vector_pool: Option<&'w VectorPoolRef>,
    pub symbol_pool: Option<&'w SymbolPoolRef>,
    pub terrain_active: bool,
    domain_slots: [Option<&'w dyn Any>; DOMAIN_SLOT_COUNT],
}

impl<'w> PassFrameRefs<'w> {
    pub(crate) const DOMAIN_EMPTY: u8 = 0;
    pub(crate) const DOMAIN_VECTOR: u8 = 1 << 0;
    pub(crate) const DOMAIN_SDF: u8 = 1 << 1;
    pub(crate) const DOMAIN_TERRAIN: u8 = 1 << 2;
    pub(crate) const DOMAIN_RASTER: u8 = 1 << 3;
    pub(crate) const DOMAIN_DRAPE: u8 = Self::DOMAIN_VECTOR | Self::DOMAIN_RASTER;

    /// Snapshots only the resource domains listed in `domains` at render-pass start.
    pub(crate) fn from_world_for(world: &'w World, domains: u8) -> Self {
        let needs_drape = (domains & Self::DOMAIN_DRAPE) != 0;
        let mut refs = Self {
            capabilities: world.resources.get::<DrawCapabilities>(),
            projection: initialized::<ProjectionGpuResources>(world),
            tile_view_pattern: needs_drape
                .then(|| initialized::<WgpuTileViewPattern>(world))
                .flatten(),
            tile_mesh_cache: world.resources.get::<GlobeTileMeshCache>(),
            mask_pipeline: needs_drape
                .then(|| initialized::<MaskPipeline>(world))
                .flatten(),
            vector_pool: ((domains & Self::DOMAIN_VECTOR) != 0)
                .then(|| initialized::<VectorPoolRef>(world))
                .flatten(),
            symbol_pool: ((domains & Self::DOMAIN_SDF) != 0)
                .then(|| initialized::<SymbolPoolRef>(world))
                .flatten(),
            terrain_active: false,
            domain_slots: [None; DOMAIN_SLOT_COUNT],
        };
        if domains != 0
            && let Some(populators) = world.resources.get::<PassRefPopulators>()
        {
            for &(domain, populate) in &populators.0 {
                if (domain & domains) != 0 {
                    populate(world, &mut refs);
                }
            }
        }
        refs
    }

    #[inline]
    pub(crate) fn set_terrain<T: 'static>(&mut self, active: bool, resources: Option<&'w T>) {
        self.terrain_active = active;
        self.set_slot(DomainPassSlot::TerrainResources, resources);
    }

    #[inline(always)]
    pub(crate) fn set_slot<T: 'static>(&mut self, slot: DomainPassSlot, value: Option<&'w T>) {
        if let Some(entry) = self.domain_slots.get_mut(slot as usize) {
            *entry = value.map(|v| v as &'w dyn Any);
        }
    }

    #[inline(always)]
    pub(crate) fn resolve_slot<T: 'static>(
        refs: Option<&Self>,
        slot: DomainPassSlot,
        fallback: impl FnOnce() -> Option<&'w T>,
    ) -> Option<&'w T> {
        match refs {
            Some(refs) => refs
                .domain_slots
                .get(slot as usize)
                .and_then(|entry| *entry)?
                .downcast_ref::<T>(),
            None => fallback(),
        }
    }

    #[inline(always)]
    pub(crate) fn resolve_vector_pool(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w VectorPoolRef> {
        refs.map_or_else(|| initialized(world), |r| r.vector_pool)
    }

    #[inline(always)]
    pub(crate) fn resolve_symbol_pool(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolPoolRef> {
        refs.map_or_else(|| initialized(world), |r| r.symbol_pool)
    }

    #[inline]
    pub(crate) fn resolve_projection(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w ProjectionGpuResources> {
        refs.map_or_else(|| initialized(world), |r| r.projection)
    }

    #[inline]
    pub(crate) fn resolve_tile_view_pattern(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w WgpuTileViewPattern> {
        refs.map_or_else(|| initialized(world), |r| r.tile_view_pattern)
    }

    #[inline]
    pub(crate) fn resolve_tile_mesh_cache(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w GlobeTileMeshCache> {
        refs.map_or_else(
            || world.resources.get::<GlobeTileMeshCache>(),
            |r| r.tile_mesh_cache,
        )
    }

    #[inline]
    pub(crate) fn resolve_mask_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w MaskPipeline> {
        refs.map_or_else(|| initialized(world), |r| r.mask_pipeline)
    }
}

/// Compact handle identity captured from `wgpu` resource `Hash` implementations without cloning
/// underlying `JsValue` / `Arc` wrappers inside `TrackedRenderPass`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::engine::render) struct HandleId {
    first: u64,
    second: u64,
    tail: u64,
}

impl HandleId {
    #[inline]
    pub(in crate::engine::render) fn of<T: Hash + ?Sized>(value: &T) -> Self {
        let mut hasher = HandleIdHasher {
            count: 0,
            first: 0,
            second: 0,
            tail: 0,
        };
        value.hash(&mut hasher);
        Self {
            first: hasher.first,
            second: hasher.second,
            tail: hasher.tail,
        }
    }
}

struct HandleIdHasher {
    count: u8,
    first: u64,
    second: u64,
    tail: u64,
}

impl HandleIdHasher {
    #[inline]
    fn push(&mut self, value: u64) {
        match self.count {
            0 => {
                self.first = value;
                self.count = 1;
            }
            1 => {
                self.second = value;
                self.count = 2;
            }
            _ => {
                self.tail = self
                    .tail
                    .rotate_left(7)
                    .wrapping_add(value)
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15);
            }
        }
    }
}

impl Hasher for HandleIdHasher {
    fn finish(&self) -> u64 {
        self.second ^ self.first.rotate_left(13) ^ self.tail
    }

    fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            self.push(0);
            return;
        }
        let len_tag = (bytes.len() as u64).rotate_left(56);
        for (idx, chunk) in bytes.chunks(8).enumerate() {
            let mut word = 0_u64;
            for (i, &byte) in chunk.iter().enumerate() {
                word |= u64::from(byte) << (i * 8);
            }
            self.push(if idx == 0 { word ^ len_tag } else { word });
        }
    }

    fn write_u8(&mut self, i: u8) {
        self.push(u64::from(i));
    }

    fn write_u32(&mut self, i: u32) {
        self.push(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.push(i);
    }

    fn write_usize(&mut self, i: usize) {
        self.push(i as u64);
    }
}

/// A bound buffer range, compared by buffer identity, offset and size.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::engine::render) struct BoundSlice {
    buffer: HandleId,
    offset: wgpu::BufferAddress,
    size: wgpu::BufferAddress,
}

impl BoundSlice {
    #[inline]
    pub(in crate::engine::render) fn of(slice: &wgpu::BufferSlice<'_>) -> Self {
        Self {
            buffer: HandleId::of(slice.buffer()),
            offset: slice.offset(),
            size: slice.size(),
        }
    }
}

pub(in crate::engine::render) type BoundBindGroup = (HandleId, SmallVec<[wgpu::DynamicOffset; 2]>);

#[inline]
pub(in crate::engine::render) fn update_bound_bind_group(
    slot: &mut Option<BoundBindGroup>,
    bg_id: HandleId,
    offsets: &[wgpu::DynamicOffset],
) -> bool {
    match slot {
        Some((bound_bg, bound_offsets)) if *bound_bg == bg_id => {
            if bound_offsets.as_slice() == offsets {
                false
            } else {
                bound_offsets.clear();
                bound_offsets.extend_from_slice(offsets);
                true
            }
        }
        _ => {
            *slot = Some((bg_id, SmallVec::from_slice(offsets)));
            true
        }
    }
}

pub(in crate::engine::render) fn update_extra_bind_group(
    extra: &mut SmallVec<[(u32, BoundBindGroup); 0]>,
    index: u32,
    bg_id: HandleId,
    offsets: &[wgpu::DynamicOffset],
) -> bool {
    if let Some((_, bound)) = extra.iter_mut().find(|(slot, _)| *slot == index) {
        if bound.0 == bg_id {
            if bound.1.as_slice() == offsets {
                false
            } else {
                bound.1.clear();
                bound.1.extend_from_slice(offsets);
                true
            }
        } else {
            *bound = (bg_id, SmallVec::from_slice(offsets));
            true
        }
    } else {
        extra.push((index, (bg_id, SmallVec::from_slice(offsets))));
        true
    }
}

#[inline]
pub(in crate::engine::render) fn update_bound_slice(
    slot: &mut Option<BoundSlice>,
    next: BoundSlice,
) -> bool {
    if *slot == Some(next) {
        false
    } else {
        *slot = Some(next);
        true
    }
}

pub(in crate::engine::render) fn update_extra_vertex_buffer(
    extra: &mut SmallVec<[(u32, BoundSlice); 0]>,
    slot_idx: u32,
    next: BoundSlice,
) -> bool {
    if let Some((_, bound)) = extra.iter_mut().find(|(slot, _)| *slot == slot_idx) {
        if *bound == next {
            false
        } else {
            *bound = next;
            true
        }
    } else {
        extra.push((slot_idx, next));
        true
    }
}

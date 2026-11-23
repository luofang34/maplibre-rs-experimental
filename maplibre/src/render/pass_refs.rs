//! Collision-free lightweight handle identifiers and pass-scoped resource snapshots so
//! `Stage::Render` avoids per-draw `wgpu` handle clones and `World::resources` hash lookups.

use std::hash::{Hash, Hasher};

use smallvec::SmallVec;

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        tile_mesh::GlobeTileMeshCache,
        tile_view_pattern::WgpuTileViewPattern,
        tracked_pass::DrawCapabilities,
        MaskPipeline,
    },
    sdf::{
        covering::SymbolCovering, depth::SymbolDepth, textures::SymbolTextures, SymbolBufferPool,
        SymbolPipeline,
    },
    tcs::world::World,
    terrain::{resources::TerrainResources, TerrainFrame},
    vector::{
        line_dash::LineDashResources, pattern::PatternResources, CirclePipeline, ExtrusionPipeline,
        LinePipeline, VectorBufferPool, VectorPipeline,
    },
};

/// Compact handle identity captured from `wgpu` resource `Hash` implementations without cloning
/// underlying `JsValue` / `Arc` wrappers inside `TrackedRenderPass`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HandleId {
    first: u64,
    second: u64,
    tail: u64,
}

impl HandleId {
    #[inline]
    pub(super) fn of<T: Hash + ?Sized>(value: &T) -> Self {
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

#[inline]
fn initialized<T: 'static>(world: &World) -> Option<&T> {
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
    pub vector_pool: Option<&'w VectorBufferPool>,
    pub vector_pipeline: Option<&'w VectorPipeline>,
    pub circle_pipeline: Option<&'w CirclePipeline>,
    pub line_pipeline: Option<&'w LinePipeline>,
    pub extrusion_pipeline: Option<&'w ExtrusionPipeline>,
    pub pattern_resources: Option<&'w PatternResources>,
    pub line_dashes: Option<&'w LineDashResources>,
    pub terrain_active: bool,
    pub terrain_resources: Option<&'w TerrainResources>,
    pub symbol_pipeline: Option<&'w SymbolPipeline>,
    pub symbol_depth: Option<&'w SymbolDepth>,
    pub symbol_pool: Option<&'w SymbolBufferPool>,
    pub symbol_covering: Option<&'w SymbolCovering>,
    pub symbol_textures: Option<&'w SymbolTextures>,
}

impl<'w> PassFrameRefs<'w> {
    /// Snapshots all shared draw-loop resources from `world` once at pass start.
    pub(crate) fn from_world(world: &'w World) -> Self {
        Self {
            capabilities: world.resources.get::<DrawCapabilities>(),
            projection: initialized::<ProjectionGpuResources>(world),
            tile_view_pattern: initialized::<WgpuTileViewPattern>(world),
            tile_mesh_cache: world.resources.get::<GlobeTileMeshCache>(),
            mask_pipeline: initialized::<MaskPipeline>(world),
            vector_pool: initialized::<VectorBufferPool>(world),
            vector_pipeline: initialized::<VectorPipeline>(world),
            circle_pipeline: initialized::<CirclePipeline>(world),
            line_pipeline: initialized::<LinePipeline>(world),
            extrusion_pipeline: initialized::<ExtrusionPipeline>(world),
            pattern_resources: world.resources.get::<PatternResources>(),
            line_dashes: world.resources.get::<LineDashResources>(),
            terrain_active: world
                .resources
                .get::<TerrainFrame>()
                .is_some_and(|frame| frame.active),
            terrain_resources: initialized::<TerrainResources>(world),
            symbol_pipeline: initialized::<SymbolPipeline>(world),
            symbol_depth: initialized::<SymbolDepth>(world),
            symbol_pool: initialized::<SymbolBufferPool>(world),
            symbol_covering: world.resources.get::<SymbolCovering>(),
            symbol_textures: world.resources.get::<SymbolTextures>(),
        }
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

    #[inline]
    pub(crate) fn resolve_vector_pool(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w VectorBufferPool> {
        refs.map_or_else(|| initialized(world), |r| r.vector_pool)
    }

    #[inline]
    pub(crate) fn resolve_vector_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w VectorPipeline> {
        refs.map_or_else(|| initialized(world), |r| r.vector_pipeline)
    }

    #[inline]
    pub(crate) fn resolve_circle_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w CirclePipeline> {
        refs.map_or_else(|| initialized(world), |r| r.circle_pipeline)
    }

    #[inline]
    pub(crate) fn resolve_line_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w LinePipeline> {
        refs.map_or_else(|| initialized(world), |r| r.line_pipeline)
    }

    #[inline]
    pub(crate) fn resolve_extrusion_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w ExtrusionPipeline> {
        refs.map_or_else(|| initialized(world), |r| r.extrusion_pipeline)
    }

    #[inline]
    pub(crate) fn resolve_pattern_resources(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w PatternResources> {
        refs.map_or_else(
            || world.resources.get::<PatternResources>(),
            |r| r.pattern_resources,
        )
    }

    #[inline]
    pub(crate) fn resolve_line_dashes(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w LineDashResources> {
        refs.map_or_else(
            || world.resources.get::<LineDashResources>(),
            |r| r.line_dashes,
        )
    }

    #[inline]
    pub(crate) fn resolve_terrain_active(refs: Option<&Self>, world: &World) -> bool {
        refs.map_or_else(
            || {
                world
                    .resources
                    .get::<TerrainFrame>()
                    .is_some_and(|frame| frame.active)
            },
            |r| r.terrain_active,
        )
    }

    #[inline]
    pub(crate) fn resolve_terrain_resources(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w TerrainResources> {
        refs.map_or_else(|| initialized(world), |r| r.terrain_resources)
    }

    #[inline]
    pub(crate) fn resolve_symbol_pipeline(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolPipeline> {
        refs.map_or_else(|| initialized(world), |r| r.symbol_pipeline)
    }

    #[inline]
    pub(crate) fn resolve_symbol_depth(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolDepth> {
        refs.map_or_else(|| initialized(world), |r| r.symbol_depth)
    }

    #[inline]
    pub(crate) fn resolve_symbol_pool(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolBufferPool> {
        refs.map_or_else(|| initialized(world), |r| r.symbol_pool)
    }

    #[inline]
    pub(crate) fn resolve_symbol_covering(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolCovering> {
        refs.map_or_else(
            || world.resources.get::<SymbolCovering>(),
            |r| r.symbol_covering,
        )
    }

    #[inline]
    pub(crate) fn resolve_symbol_textures(
        refs: Option<&Self>,
        world: &'w World,
    ) -> Option<&'w SymbolTextures> {
        refs.map_or_else(
            || world.resources.get::<SymbolTextures>(),
            |r| r.symbol_textures,
        )
    }
}

/// A bound buffer range, compared by buffer identity, offset and size.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BoundSlice {
    buffer: HandleId,
    offset: wgpu::BufferAddress,
    size: wgpu::BufferAddress,
}

impl BoundSlice {
    #[inline]
    pub(super) fn of(slice: &wgpu::BufferSlice<'_>) -> Self {
        Self {
            buffer: HandleId::of(slice.buffer()),
            offset: slice.offset(),
            size: slice.size(),
        }
    }
}

pub(super) type BoundBindGroup = (HandleId, SmallVec<[wgpu::DynamicOffset; 2]>);

#[inline]
pub(super) fn update_bound_bind_group(
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

pub(super) fn update_extra_bind_group(
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
pub(super) fn update_bound_slice(slot: &mut Option<BoundSlice>, next: BoundSlice) -> bool {
    if *slot == Some(next) {
        false
    } else {
        *slot = Some(next);
        true
    }
}

pub(super) fn update_extra_vertex_buffer(
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

//! Dirty sub-range diffing and coalesced GPU uploads for `SDFShaderFeatureMetadata`.
//!
//! Newly allocated symbol buckets are zero-initialized in GPU memory (`SDFShaderFeatureMetadata::default()`).
//! During a `PlacementEpoch`, most symbols in a tile either stay clean or remain hidden, so
//! diffing per-vertex metadata and uploading only coalesced dirty runs eliminates ~90% of
//! `queue.writeBuffer` bandwidth compared to rewriting entire bucket allocations.

use bytemuck::Pod;

use crate::{
    render::{resource::Queue, shaders::SDFShaderFeatureMetadata},
    vector::resource::{BufferPool, IndexEntry},
};

/// Clean quads tolerated between two dirty runs before splitting into two `writeBuffer` calls.
/// `8 * 4 * 80 B = 2,560 B`, balancing fewer WebGPU IPC calls against minimal clean-byte over-upload.
const MAX_CLEAN_GAP_QUADS: usize = 8;

/// Maximum quads expanded or zeroed per chunk upload (`1024 * 4 * 80 B = 320 KiB` scratch cap).
const MAX_UPLOAD_QUADS_PER_CHUNK: usize = 1024;

/// Reusable pool of per-bucket per-quad `SDFShaderFeatureMetadata` vectors and bounded upload
/// staging slices so placement epochs do not churn multi-hundred-kilobyte heap allocations.
#[derive(Default)]
pub(super) struct MetadataScratchPool {
    free: Vec<Vec<SDFShaderFeatureMetadata>>,
    expand: Vec<SDFShaderFeatureMetadata>,
    zeros: Vec<SDFShaderFeatureMetadata>,
}

impl MetadataScratchPool {
    /// Pops a cleared per-quad `Vec<SDFShaderFeatureMetadata>` with retained capacity when available.
    pub(super) fn acquire(&mut self) -> Vec<SDFShaderFeatureMetadata> {
        let mut buf = self.free.pop().unwrap_or_default();
        buf.clear();
        buf
    }

    /// Returns a per-quad `Vec<SDFShaderFeatureMetadata>` to the pool if capacity is bounded.
    pub(super) fn recycle(&mut self, mut buf: Vec<SDFShaderFeatureMetadata>) {
        if buf.capacity() > 0 && buf.capacity() <= 16_384 && self.free.len() < 2 {
            buf.clear();
            self.free.push(buf);
        }
    }

    fn expand_quads(&mut self, quads: &[SDFShaderFeatureMetadata]) -> &[SDFShaderFeatureMetadata] {
        self.expand.clear();
        self.expand.reserve(quads.len() * 4);
        for &quad in quads {
            self.expand.extend_from_slice(&[quad; 4]);
        }
        &self.expand
    }

    fn zero_slice(&mut self, len: usize) -> &[SDFShaderFeatureMetadata] {
        if self.zeros.len() < len {
            self.zeros.resize(len, SDFShaderFeatureMetadata::default());
        }
        &self.zeros[..len]
    }
}

/// Last GPU-resident per-quad `SDFShaderFeatureMetadata` state for one bucket.
#[derive(Clone, Debug, Default)]
pub(super) struct CachedBucketMetadata {
    /// `IndexEntry::allocation_id()` of the backing GPU buffer slice.
    pub allocation_id: u64,
    /// Whether at least one quad in `quads` has target `opacity > 0.0`.
    pub has_target_visible: bool,
    /// Latest `start_time_ms` among quads with `start_opacity > 0.0`, or `-1.0` when none.
    pub latest_fade_start_ms: f32,
    /// One entry per 4-vertex quad; empty when all GPU vertices are default-zeroed.
    pub quads: Vec<SDFShaderFeatureMetadata>,
}

impl CachedBucketMetadata {
    fn empty(allocation_id: u64) -> Self {
        Self {
            allocation_id,
            has_target_visible: false,
            latest_fade_start_ms: -1.0,
            quads: Vec::new(),
        }
    }

    /// Whether any vertex in this bucket evaluates to non-zero opacity at `now_ms`.
    pub(super) fn is_active_at(&self, now_ms: f32, fade_duration_ms: f32) -> bool {
        self.has_target_visible
            || (fade_duration_ms > 0.0
                && self.latest_fade_start_ms >= 0.0
                && now_ms < self.latest_fade_start_ms + fade_duration_ms)
    }
}

/// Diffs `next` against `prev` (treating a fresh `allocation_id` as all-zero in GPU memory) and
/// writes only coalesced dirty vertex ranges to `pool`.
#[cfg(test)]
pub(super) fn diff_and_upload_bucket<
    Q: Queue<B>,
    B,
    V: crate::render::shaders::PoolVertex,
    I: Pod,
    TM: Pod,
>(
    pool: &BufferPool<Q, B, V, I, TM, SDFShaderFeatureMetadata>,
    queue: &Q,
    entry: &IndexEntry,
    prev: Option<&CachedBucketMetadata>,
    next: Vec<SDFShaderFeatureMetadata>,
) -> CachedBucketMetadata {
    let mut scratch = MetadataScratchPool::default();
    diff_and_upload_bucket_with_scratch(pool, queue, entry, prev, next, &mut scratch)
}

/// Diffs `next` against `prev` and recycles unused `SDFShaderFeatureMetadata` buffers into `scratch`.
pub(super) fn diff_and_upload_bucket_with_scratch<
    Q: Queue<B>,
    B,
    V: crate::render::shaders::PoolVertex,
    I: Pod,
    TM: Pod,
>(
    pool: &BufferPool<Q, B, V, I, TM, SDFShaderFeatureMetadata>,
    queue: &Q,
    entry: &IndexEntry,
    prev: Option<&CachedBucketMetadata>,
    next: Vec<SDFShaderFeatureMetadata>,
    scratch: &mut MetadataScratchPool,
) -> CachedBucketMetadata {
    let allocation_id = entry.allocation_id();
    let stride = size_of::<SDFShaderFeatureMetadata>() as u64;
    let range = entry.feature_metadata_buffer_range();
    let allocated_quads = (((range.end - range.start) / stride) as usize) / 4;
    if allocated_quads == 0 {
        scratch.recycle(next);
        return CachedBucketMetadata::empty(allocation_id);
    }

    let prev_slice = prev
        .filter(|cached| cached.allocation_id == allocation_id)
        .map_or(&[][..], |cached| cached.quads.as_slice());

    // Fast path 1: all symbols hidden on a newly zeroed allocation or already-zeroed bucket.
    if next.is_empty() && prev_slice.is_empty() {
        scratch.recycle(next);
        return CachedBucketMetadata::empty(allocation_id);
    }

    // Fast path 2: bucket became completely hidden after having visible symbols.
    if next.is_empty() {
        scratch.recycle(next);
        let zero = SDFShaderFeatureMetadata::default();
        let zero_bytes = bytemuck::bytes_of(&zero);
        let limit = prev_slice.len().min(allocated_quads);
        for (start, end) in dirty_runs(limit, |i| bytemuck::bytes_of(&prev_slice[i]) != zero_bytes)
        {
            let mut cursor = start;
            while cursor < end {
                let chunk_end = (cursor + MAX_UPLOAD_QUADS_PER_CHUNK).min(end);
                let zeros = scratch.zero_slice((chunk_end - cursor) * 4);
                pool.update_feature_metadata_range(queue, entry, cursor * 4, zeros);
                cursor = chunk_end;
            }
        }
        return CachedBucketMetadata::empty(allocation_id);
    }

    let mut next = next;
    if next.len() > allocated_quads {
        next.truncate(allocated_quads);
    }
    let zero = SDFShaderFeatureMetadata::default();
    let limit = next.len();
    for (start, end) in dirty_runs(limit, |i| {
        let old = prev_slice.get(i).unwrap_or(&zero);
        bytemuck::bytes_of(old) != bytemuck::bytes_of(&next[i])
    }) {
        let mut cursor = start;
        while cursor < end {
            let chunk_end = (cursor + MAX_UPLOAD_QUADS_PER_CHUNK).min(end);
            let expanded = scratch.expand_quads(&next[cursor..chunk_end]);
            pool.update_feature_metadata_range(queue, entry, cursor * 4, expanded);
            cursor = chunk_end;
        }
    }
    if prev_slice.len() > limit {
        let tail_end = prev_slice.len().min(allocated_quads);
        let zero_bytes = bytemuck::bytes_of(&zero);
        for (rel_start, rel_end) in dirty_runs(tail_end - limit, |i| {
            bytemuck::bytes_of(&prev_slice[limit + i]) != zero_bytes
        }) {
            let mut cursor = rel_start;
            while cursor < rel_end {
                let chunk_end = (cursor + MAX_UPLOAD_QUADS_PER_CHUNK).min(rel_end);
                let zeros = scratch.zero_slice((chunk_end - cursor) * 4);
                pool.update_feature_metadata_range(queue, entry, (limit + cursor) * 4, zeros);
                cursor = chunk_end;
            }
        }
    }

    let mut has_target_visible = false;
    let mut latest_fade_start_ms = -1.0_f32;
    for quad in &next {
        if quad.has_target_opacity() {
            has_target_visible = true;
        }
        if quad.has_start_opacity() {
            latest_fade_start_ms = latest_fade_start_ms.max(quad.start_time_ms());
        }
    }

    CachedBucketMetadata {
        allocation_id,
        has_target_visible,
        latest_fade_start_ms,
        quads: next,
    }
}

fn dirty_runs(len: usize, mut is_dirty: impl FnMut(usize) -> bool) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < len {
        if !is_dirty(i) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < len && is_dirty(i) {
            i += 1;
        }
        let end = i;
        if let Some(last) = runs.last_mut() {
            if start - last.1 <= MAX_CLEAN_GAP_QUADS {
                last.1 = end;
                continue;
            }
        }
        runs.push((start, end));
    }
    runs
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        render::{
            resource::BackingBufferDescriptor,
            shaders::{ShaderLayerMetadata, ShaderSymbolVertex},
        },
        style::layer::StyleLayer,
        vector::{resource::BufferPool, tessellation::OverAlignedVertexBuffer},
    };

    #[derive(Default)]
    struct RecordingQueue {
        writes: RefCell<Vec<(u64, usize)>>,
    }

    impl Queue<u32> for RecordingQueue {
        fn write_buffer(&self, _buffer: &u32, offset: wgpu::BufferAddress, data: &[u8]) {
            self.writes.borrow_mut().push((offset, data.len()));
        }
    }

    fn test_pool_with_bucket(
        vertex_count: usize,
    ) -> (
        BufferPool<
            RecordingQueue,
            u32,
            ShaderSymbolVertex,
            u32,
            ShaderLayerMetadata,
            SDFShaderFeatureMetadata,
        >,
        RecordingQueue,
        WorldTileCoords,
    ) {
        let mut pool = BufferPool::new(
            BackingBufferDescriptor::new(1_u32, 64 * 1024),
            BackingBufferDescriptor::new(2_u32, 64 * 1024),
            BackingBufferDescriptor::new(3_u32, 4 * 1024),
            BackingBufferDescriptor::new(4_u32, 64 * 1024),
        );
        let queue = RecordingQueue::default();
        let coords = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(10),
        };
        let mut buffer = OverAlignedVertexBuffer::empty();
        buffer.buffer.vertices = vec![ShaderSymbolVertex::default(); vertex_count];
        buffer.buffer.indices = vec![0, 1, 2, 0, 2, 3];
        buffer.usable_indices = 6;
        let style_layer = StyleLayer {
            id: "labels".into(),
            type_: "symbol".into(),
            ..Default::default()
        };
        pool.replace_layer_geometry_uniform_unless(
            &queue,
            coords,
            style_layer,
            &buffer,
            ShaderLayerMetadata::new(0.0, 0.0, [0.0; 2]),
            SDFShaderFeatureMetadata::default(),
            vertex_count,
            |_| false,
        )
        .expect("allocate test symbol bucket");
        queue.writes.borrow_mut().clear();
        (pool, queue, coords)
    }

    #[test]
    fn initial_sparse_placement_uploads_only_placed_quad_subrange() {
        let (pool, queue, coords) = test_pool_with_bucket(200);
        let entry = &pool.index().get_layers(coords).expect("layers")[0];
        let stride = size_of::<SDFShaderFeatureMetadata>();

        // Only 1 quad (quad 25 => vertices 100..104) is placed out of 50 quads (200 vertices).
        let mut next = vec![SDFShaderFeatureMetadata::default(); 50];
        next[25].set_target_opacity(1.0);
        let cached = diff_and_upload_bucket(&pool, &queue, entry, None, next.clone());
        assert_eq!(
            *queue.writes.borrow(),
            vec![((100 * stride) as u64, 4 * stride)]
        );

        // Identical follow-up epoch issues 0 writes.
        queue.writes.borrow_mut().clear();
        let cached = diff_and_upload_bucket(&pool, &queue, entry, Some(&cached), next.clone());
        assert!(queue.writes.borrow().is_empty());

        // Hiding that single quad zeroes only those 4 vertices.
        let cached = diff_and_upload_bucket(&pool, &queue, entry, Some(&cached), Vec::new());
        assert_eq!(
            *queue.writes.borrow(),
            vec![((100 * stride) as u64, 4 * stride)]
        );
        assert!(cached.quads.is_empty());
    }

    #[test]
    fn nearby_dirty_quads_coalesce_while_distant_quads_split() {
        let (pool, queue, coords) = test_pool_with_bucket(200);
        let entry = &pool.index().get_layers(coords).expect("layers")[0];
        let stride = size_of::<SDFShaderFeatureMetadata>();

        let mut next = vec![SDFShaderFeatureMetadata::default(); 50];
        // Quads 2 (8..12) and 5 (20..24) are separated by 2 clean quads (<= 8) -> coalesced!
        // Quad 30 (120..124) is 24 clean quads away (> 8) -> separate sub-range!
        for idx in [2, 5, 30] {
            next[idx].set_target_opacity(1.0);
        }
        let _ = diff_and_upload_bucket(&pool, &queue, entry, None, next);
        assert_eq!(
            *queue.writes.borrow(),
            vec![
                ((8 * stride) as u64, 16 * stride),
                ((120 * stride) as u64, 4 * stride),
            ]
        );
    }
}

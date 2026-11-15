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

/// Clean vertices tolerated between two dirty runs before splitting into two `writeBuffer` calls.
/// `32 * 80 B = 2,560 B`, balancing fewer WebGPU IPC calls against minimal clean-byte over-upload.
const MAX_CLEAN_GAP_VERTICES: usize = 32;

/// Last GPU-resident `SDFShaderFeatureMetadata` state for one `(WorldTileCoords, style_layer_id)` bucket.
#[derive(Clone, Debug, Default)]
pub(super) struct CachedBucketMetadata {
    /// `IndexEntry::allocation_id()` of the backing GPU buffer slice.
    pub allocation_id: u64,
    /// Empty when all GPU vertices in this allocation are `SDFShaderFeatureMetadata::default()`.
    pub vertices: Vec<SDFShaderFeatureMetadata>,
}

/// Diffs `next` against `prev` (treating a fresh `allocation_id` as all-zero in GPU memory) and
/// writes only coalesced dirty vertex ranges to `pool`.
pub(super) fn diff_and_upload_bucket<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod>(
    pool: &BufferPool<Q, B, V, I, TM, SDFShaderFeatureMetadata>,
    queue: &Q,
    entry: &IndexEntry,
    prev: Option<&CachedBucketMetadata>,
    next: Vec<SDFShaderFeatureMetadata>,
) -> CachedBucketMetadata {
    let allocation_id = entry.allocation_id();
    let stride = size_of::<SDFShaderFeatureMetadata>() as u64;
    let range = entry.feature_metadata_buffer_range();
    let allocated_vertices = ((range.end - range.start) / stride) as usize;
    if allocated_vertices == 0 {
        return CachedBucketMetadata {
            allocation_id,
            vertices: Vec::new(),
        };
    }

    let prev_slice = prev
        .filter(|cached| cached.allocation_id == allocation_id)
        .map_or(&[][..], |cached| cached.vertices.as_slice());

    // Fast path 1: all symbols hidden on a newly zeroed allocation or already-zeroed bucket.
    if next.is_empty() && prev_slice.is_empty() {
        return CachedBucketMetadata {
            allocation_id,
            vertices: Vec::new(),
        };
    }

    // Fast path 2: bucket became completely hidden after having visible symbols.
    if next.is_empty() {
        let zero = SDFShaderFeatureMetadata::default();
        let zero_bytes = bytemuck::bytes_of(&zero);
        let limit = prev_slice.len().min(allocated_vertices);
        for (start, end) in dirty_runs(limit, |i| bytemuck::bytes_of(&prev_slice[i]) != zero_bytes)
        {
            let zeros = vec![zero; end - start];
            pool.update_feature_metadata_range(queue, entry, start, &zeros);
        }
        return CachedBucketMetadata {
            allocation_id,
            vertices: Vec::new(),
        };
    }

    let mut next = next;
    if next.len() > allocated_vertices {
        next.truncate(allocated_vertices);
    }
    let zero = SDFShaderFeatureMetadata::default();
    let limit = next.len();
    for (start, end) in dirty_runs(limit, |i| {
        let old = prev_slice.get(i).unwrap_or(&zero);
        bytemuck::bytes_of(old) != bytemuck::bytes_of(&next[i])
    }) {
        pool.update_feature_metadata_range(queue, entry, start, &next[start..end]);
    }
    if prev_slice.len() > limit {
        let tail_end = prev_slice.len().min(allocated_vertices);
        let zero_bytes = bytemuck::bytes_of(&zero);
        for (rel_start, rel_end) in dirty_runs(tail_end - limit, |i| {
            bytemuck::bytes_of(&prev_slice[limit + i]) != zero_bytes
        }) {
            let zeros = vec![zero; rel_end - rel_start];
            pool.update_feature_metadata_range(queue, entry, limit + rel_start, &zeros);
        }
    }

    CachedBucketMetadata {
        allocation_id,
        vertices: next,
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
            if start - last.1 <= MAX_CLEAN_GAP_VERTICES {
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
        buffer.buffer.vertices = vec![
            ShaderSymbolVertex {
                a_pos_offset: [0; 4],
                a_data: [0; 4],
                a_pixeloffset: [0; 4],
            };
            vertex_count
        ];
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

        // Only 1 quad (vertices 100..104) is placed out of 200 vertices.
        let mut next = vec![SDFShaderFeatureMetadata::default(); 200];
        for v in &mut next[100..104] {
            v.opacity = 1.0;
        }
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
        assert!(cached.vertices.is_empty());
    }

    #[test]
    fn nearby_dirty_quads_coalesce_while_distant_quads_split() {
        let (pool, queue, coords) = test_pool_with_bucket(200);
        let entry = &pool.index().get_layers(coords).expect("layers")[0];
        let stride = size_of::<SDFShaderFeatureMetadata>();

        let mut next = vec![SDFShaderFeatureMetadata::default(); 200];
        // Quads at 10..14 and 20..24 are separated by 6 clean vertices (<= 32) -> coalesced!
        // Quad at 120..124 is 96 clean vertices away (> 32) -> separate sub-range!
        for idx in (10..14).chain(20..24).chain(120..124) {
            next[idx].opacity = 1.0;
        }
        let _ = diff_and_upload_bucket(&pool, &queue, entry, None, next);
        assert_eq!(
            *queue.writes.borrow(),
            vec![
                ((10 * stride) as u64, 14 * stride),
                ((120 * stride) as u64, 4 * stride),
            ]
        );
    }
}

//! Flushes queued per-bucket symbol metadata diffs to the GPU pool.

use std::collections::VecDeque;

use rustc_hash::FxHashMap;

use super::upload_diff;
use crate::{
    coords::WorldTileCoords,
    render::{shaders::SDFShaderFeatureMetadata, upload_queue::UploadQueue},
    sdf::SymbolBufferPool,
    style::StyleLayerId,
};

pub(super) fn flush_pending_uploads(
    pool: &SymbolBufferPool,
    pending_uploads: &mut VecDeque<(StyleLayerId, WorldTileCoords, Vec<SDFShaderFeatureMetadata>)>,
    queue: &UploadQueue,
    uploaded: &mut FxHashMap<(WorldTileCoords, StyleLayerId), upload_diff::CachedBucketMetadata>,
    scratch: &mut upload_diff::MetadataScratchPool,
) {
    while let Some((layer_id, coords, metadata)) = pending_uploads.pop_front() {
        let Some(entry) = pool.as_ref().get_layers(coords).and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.style_layer_id == layer_id)
        }) else {
            scratch.recycle(metadata);
            continue;
        };
        let key = (coords, layer_id);
        let prev = uploaded.remove(&key);
        let next_cached = upload_diff::diff_and_upload_bucket_with_scratch(
            pool,
            queue,
            entry,
            prev.as_ref(),
            metadata,
            scratch,
        );
        if let Some(prev) = prev {
            scratch.recycle(prev.quads);
        }
        uploaded.insert(key, next_cached);
    }
}

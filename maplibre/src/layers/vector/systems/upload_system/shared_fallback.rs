//! Lane-scoped shared vertex fallback, safe CPU geometry release, and missing-vertex checks.

use crate::{
    coords::WorldTileCoords,
    style::layer::StyleLayer,
    tcs::tiles::Tiles,
    vector::{
        content::{release_committed_cpu_geometry, LayerReplacements},
        AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

pub(super) fn can_release_cpu_geometry(
    style_layer: &StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    has_dem_source: bool,
) -> bool {
    let z = u8::from(bucket.coords.z);
    let edge_y = (1_i32 << z.min(30)) - 1;
    let is_polar_row = bucket.coords.y <= 0 || bucket.coords.y >= edge_y;
    let allow_row = z > 0 && !(has_dem_source && is_polar_row && style_layer.type_ == "fill");
    allow_row
        && (!has_dem_source || crate::vector::structures::kind(style_layer).is_none())
        && style_layer
            .paint
            .as_ref()
            .and_then(crate::vector::pattern::per_feature_pattern)
            .is_none()
        && !(style_layer.type_ == "circle" && !bucket.feature_sort_keys.is_empty())
}

/// Finds raw sibling CPU vertices within the same upload lane (`pending.vector` when
/// `in_pending_lane` is true, or committed `layers.layers` otherwise) so a replacement attempt
/// never borrows vertices from an older committed generation.
pub(super) fn find_shared_fallback_vertices<'a>(
    bucket: &AvailableVectorLayerBucket,
    layers: &'a VectorLayerBucketComponent,
    pending: &'a LayerReplacements,
    in_pending_lane: bool,
) -> Option<&'a [crate::render::ShaderVertex]> {
    let shared_key = bucket
        .buffer
        .shared_geometry_key()
        .filter(|_| !bucket.buffer.has_cpu_vertices())?;
    if in_pending_lane {
        pending.vector.iter().find(|sib| {
            sib.buffer.shared_geometry_key() == Some(shared_key) && !sib.buffer.vertices.is_empty()
        })
    } else {
        layers.layers.iter().find_map(|sib| match sib {
            VectorLayerBucket::AvailableLayer(sib)
                if sib.buffer.shared_geometry_key() == Some(shared_key)
                    && !sib.buffer.vertices.is_empty() =>
            {
                Some(sib)
            }
            _ => None,
        })
    }
    .map(|sib| sib.buffer.vertices.as_slice())
}

/// Returns `true` when `bucket` needs shared vertices that are absent from CPU memory, lane
/// fallback siblings, and `buffer_pool`'s resident shared vertex slabs.
pub(super) fn missing_shared_vertices(
    buffer_pool: &VectorBufferPool,
    coords: WorldTileCoords,
    style_layer: &StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    fallback_vertices: Option<&[crate::render::ShaderVertex]>,
) -> bool {
    bucket.buffer.usable_indices > 0
        && !bucket.buffer.has_cpu_vertices()
        && fallback_vertices.is_none()
        && bucket
            .buffer
            .shared_geometry_key()
            .is_some_and(|key| !buffer_pool.has_shared_vertex_slot(coords, key, &style_layer.type_))
}

pub(super) fn flush_safe_cpu_releases(
    tiles: &mut Tiles,
    buffer_pool: &VectorBufferPool,
    coords: WorldTileCoords,
    releases: &[(String, Option<u64>, bool)],
) {
    for (layer_id, shared_key, release_colors) in releases {
        let sibling_pending = shared_key.is_some_and(|key| {
            tiles
                .query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
                .is_some_and(|(layers, pending)| {
                    pending.vector.iter().any(|sib| {
                        sib.buffer.shared_geometry_key() == Some(key)
                            && sib.buffer.has_cpu_indices()
                    }) || layers.layers.iter().any(|sib| match sib {
                        VectorLayerBucket::AvailableLayer(sib) => {
                            sib.buffer.shared_geometry_key() == Some(key)
                                && sib.buffer.has_cpu_indices()
                                && !buffer_pool.has_layer(coords, &sib.style_layer_id)
                        }
                        _ => false,
                    })
                })
        });
        if !sibling_pending {
            release_committed_cpu_geometry(tiles, coords, layer_id, *release_colors);
        }
    }
}

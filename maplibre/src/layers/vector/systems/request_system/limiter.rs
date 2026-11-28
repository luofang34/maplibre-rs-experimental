//! Vector upload permit reconciliation against GPU pool uploads.

pub(crate) use crate::io::source_client::limiter::*;

pub(crate) fn clear_upload_permits(world: &mut crate::tcs::world::World) {
    if let Some(permits) = world.resources.get_mut::<PendingUploadPermits>() {
        permits.clear();
    }
}

pub(crate) fn snapshot_permit_sources(
    world: &crate::tcs::world::World,
    active: bool,
    source_tiles: &[crate::coords::WorldTileCoords],
) -> Option<rustc_hash::FxHashSet<crate::coords::WorldTileCoords>> {
    (active
        && world
            .resources
            .get::<PendingUploadPermits>()
            .is_some_and(|permits| !permits.is_empty()))
    .then(|| source_tiles.iter().copied().collect())
}

pub(crate) fn reconcile_upload_permits(
    world: &mut crate::tcs::world::World,
    deferred: bool,
    style_revision: u64,
    permit_source_set: Option<&rustc_hash::FxHashSet<crate::coords::WorldTileCoords>>,
) {
    use crate::{
        render::eventually::Eventually::Initialized,
        vector::{content::LayerReplacements, VectorBufferPool, VectorLayerBucketComponent},
    };
    let Some(mut permits) = world.resources.remove::<PendingUploadPermits>() else {
        return;
    };
    let end_stamp = deferred.then(|| {
        world
            .resources
            .get::<crate::render::eventually::Eventually<VectorBufferPool>>()
            .and_then(|pool| match pool {
                Initialized(pool) => Some((style_revision, pool.eviction_generation())),
                _ => None,
            })
    });
    permits.retain_deferred(deferred && permit_source_set.is_some(), |coords| {
        permit_source_set.is_some_and(|set| set.contains(&coords))
            && world
                .tiles
                .query::<(&VectorLayerBucketComponent, &LayerReplacements)>(coords)
                .is_some_and(|(layers, pending)| {
                    layers.done
                        && (!pending.vector.is_empty()
                            || end_stamp
                                .flatten()
                                .is_some_and(|stamp| layers.uploaded_stamp != Some(stamp)))
                })
    });
    world.resources.insert(permits);
}

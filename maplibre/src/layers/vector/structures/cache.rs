use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use super::{kind, prepare, SpatialBuffer, StructureKind};
use crate::{
    coords::WorldTileCoords,
    render::eventually::{Eventually, Eventually::Initialized},
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{coverage::TerrainCoverageIndex, source::dem_source, DemTileComponent},
    vector::{content, VectorBufferPool},
};

type PrecheckKey = (u64, u64, u64, u64, usize, u64, u64);

#[derive(Default)]
struct CachedProfiles {
    key: u64,
    precheck_key: Option<PrecheckKey>,
    buffers: Arc<Vec<SpatialBuffer>>,
    style_rev: Option<u64>,
    style_digest: u64,
    has_markers: bool,
    has_non_overlay: bool,
    structure_layer_ids: Arc<rustc_hash::FxHashSet<String>>,
}

fn digest_style_structures(style: &Style) -> (u64, bool, bool, Arc<rustc_hash::FxHashSet<String>>) {
    let mut hash = rustc_hash::FxHasher::default();
    style
        .terrain
        .as_ref()
        .map(|terrain| terrain.exaggeration.to_bits())
        .hash(&mut hash);
    let mut has_markers = false;
    let mut has_non_overlay = false;
    let mut structure_layer_ids = rustc_hash::FxHashSet::default();
    for layer in &style.layers {
        let layer_kind = kind(layer);
        if matches!(layer_kind, Some(StructureKind::Marker)) {
            has_markers = true;
        }
        if layer_kind.is_some_and(|k| !matches!(k, StructureKind::Overlay)) {
            has_non_overlay = true;
            structure_layer_ids.insert(layer.id.clone());
        }
        layer.id.hash(&mut hash);
        layer_kind.hash(&mut hash);
        if let Some(metadata) = &layer.metadata {
            let mut entries: Vec<_> = metadata.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            entries.hash(&mut hash);
        }
    }
    (
        hash.finish(),
        has_markers,
        has_non_overlay,
        Arc::new(structure_layer_ids),
    )
}

fn marker_drawn_signature(drawn: Option<&TerrainCoverageIndex>) -> u64 {
    let Some(drawn) = drawn else {
        return 0;
    };
    let mut count = 0_u64;
    let mut xor = 0_u64;
    let mut sum = 0_u64;
    for coords in drawn.rendered_tiles() {
        let mut h = rustc_hash::FxHasher::default();
        (coords, drawn.loaded_dem_for(coords)).hash(&mut h);
        let tile_hash = h.finish();
        count += 1;
        xor ^= tile_hash;
        sum = sum.wrapping_add(tile_hash);
    }
    let mut out = rustc_hash::FxHasher::default();
    (count, xor, sum).hash(&mut out);
    out.finish()
}

fn hash_sources(sources: &[WorldTileCoords]) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    sources.len().hash(&mut h);
    for coords in sources {
        coords.hash(&mut h);
    }
    h.finish()
}

fn vector_pool_revision(world: &World) -> u64 {
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => pool.revision(),
        _ => 0,
    }
}

fn fast_precheck_key(
    world: &World,
    sources: &[WorldTileCoords],
    drawn: Option<&TerrainCoverageIndex>,
    style_rev: u64,
    has_markers: bool,
) -> Option<PrecheckKey> {
    let dem_epoch = crate::terrain::dem_arrival_epoch(world);
    let vec_epoch = crate::vector::arrival_epoch(world);
    if dem_epoch == 0 || vec_epoch == 0 {
        return None;
    }
    let pool_rev = vector_pool_revision(world);
    let tiles_len = world.tiles.len();
    let sources_hash = hash_sources(sources);
    let marker_sig = if has_markers {
        marker_drawn_signature(drawn)
    } else {
        0
    };
    Some((
        style_rev,
        dem_epoch,
        vec_epoch,
        pool_rev,
        tiles_len,
        sources_hash,
        marker_sig,
    ))
}

struct EmptyStyleMeta {
    rev: u64,
    digest: u64,
    has_markers: bool,
    has_non_overlay: bool,
    layer_ids: Arc<rustc_hash::FxHashSet<String>>,
}

fn empty_profiles(
    world: &mut World,
    style_meta: Option<EmptyStyleMeta>,
) -> Arc<Vec<SpatialBuffer>> {
    if style_meta.is_none() {
        if let Some(cached) = world
            .resources
            .get::<CachedProfiles>()
            .filter(|cached| cached.key == 0)
        {
            return cached.buffers.clone();
        }
    }
    let buffers = world
        .resources
        .get::<CachedProfiles>()
        .filter(|cached| cached.key == 0)
        .map_or_else(|| Arc::new(Vec::new()), |cached| cached.buffers.clone());
    let (style_rev, style_digest, has_markers, has_non_overlay, structure_layer_ids) =
        match style_meta {
            Some(meta) => (
                Some(meta.rev),
                meta.digest,
                meta.has_markers,
                meta.has_non_overlay,
                meta.layer_ids,
            ),
            None => (None, 0, false, false, Arc::default()),
        };
    world.resources.insert(CachedProfiles {
        key: 0,
        precheck_key: None,
        buffers: buffers.clone(),
        style_rev,
        style_digest,
        has_markers,
        has_non_overlay,
        structure_layer_ids,
    });
    buffers
}

type StyleStructuresMeta = (u64, bool, Arc<rustc_hash::FxHashSet<String>>);

fn resolve_style_structures(
    world: &mut World,
    style: &Style,
    style_rev: u64,
) -> Result<StyleStructuresMeta, Arc<Vec<SpatialBuffer>>> {
    if let Some(cached) = world
        .resources
        .get::<CachedProfiles>()
        .filter(|c| c.style_rev == Some(style_rev))
    {
        if !cached.has_non_overlay && cached.key == 0 {
            return Err(cached.buffers.clone());
        }
        return Ok((
            cached.style_digest,
            cached.has_markers,
            Arc::clone(&cached.structure_layer_ids),
        ));
    }
    let (style_digest, has_markers, has_non_overlay, structure_layer_ids) =
        digest_style_structures(style);
    if !has_non_overlay {
        let meta = Some(EmptyStyleMeta {
            rev: style_rev,
            digest: style_digest,
            has_markers,
            has_non_overlay: false,
            layer_ids: structure_layer_ids,
        });
        return Err(empty_profiles(world, meta));
    }
    Ok((style_digest, has_markers, structure_layer_ids))
}

pub(crate) fn for_frame(
    world: &mut World,
    style: &Style,
    sources: &[WorldTileCoords],
) -> (Arc<Vec<SpatialBuffer>>, bool) {
    if dem_source(style).is_none() {
        return (empty_profiles(world, None), false);
    }
    let style_rev = style.revision();
    let (style_digest, has_markers, structure_layer_ids) =
        match resolve_style_structures(world, style, style_rev) {
            Ok(meta) => meta,
            Err(empty) => return (empty, false),
        };
    // The terrain this frame draws, which markers stand on.
    let (precheck, key, hit_buffers) = {
        let drawn = world.resources.get::<TerrainCoverageIndex>();
        let precheck = fast_precheck_key(world, sources, drawn, style_rev, has_markers);
        if let Some(precheck_val) = precheck
            && let Some(cached) = world
                .resources
                .get::<CachedProfiles>()
                .filter(|c| c.precheck_key == Some(precheck_val))
        {
            return (cached.buffers.clone(), false);
        }
        let key = fingerprint(
            &world.tiles,
            sources,
            drawn,
            (style_digest, has_markers, &structure_layer_ids),
        );
        let hit = world
            .resources
            .get::<CachedProfiles>()
            .filter(|cached| cached.key == key && cached.style_rev == Some(style_rev))
            .map(|cached| cached.buffers.clone());
        (precheck, key, hit)
    };
    if let Some(buffers) = hit_buffers {
        if let Some(cached) = world.resources.get_mut::<CachedProfiles>() {
            cached.precheck_key = precheck;
        }
        return (buffers, false);
    }
    let drawn = world.resources.get::<TerrainCoverageIndex>();
    let buffers = Arc::new(prepare(&world.tiles, style, sources, drawn));
    world.resources.insert(CachedProfiles {
        key,
        precheck_key: precheck,
        buffers: buffers.clone(),
        style_rev: Some(style_rev),
        style_digest,
        has_markers,
        has_non_overlay: true,
        structure_layer_ids,
    });
    (buffers, true)
}

fn fingerprint(
    tiles: &Tiles,
    sources: &[WorldTileCoords],
    drawn: Option<&TerrainCoverageIndex>,
    (style_digest, has_markers, structure_layer_ids): (u64, bool, &rustc_hash::FxHashSet<String>),
) -> u64 {
    let mut hash = rustc_hash::FxHasher::default();
    style_digest.hash(&mut hash);
    // A marker moves when the drawn terrain switches DEM tiles, even if none has loaded.
    if has_markers {
        marker_drawn_signature(drawn).hash(&mut hash);
    }
    for coords in tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
    {
        if let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) {
            coords.hash(&mut hash);
            dem.revision.hash(&mut hash);
            dem.min().to_bits().hash(&mut hash);
            dem.max().to_bits().hash(&mut hash);
        }
    }
    for coords in sources {
        coords.hash(&mut hash);
        if let Some(pending) = tiles.query::<&content::LayerReplacements>(*coords) {
            pending.revision.hash(&mut hash);
        }
        for bucket in content::vector_layers(tiles, *coords) {
            if !structure_layer_ids.contains(&bucket.style_layer_id) {
                continue;
            }
            bucket.style_layer_id.hash(&mut hash);
            bucket.buffer.vertices.len().hash(&mut hash);
            if let Some(first) = bucket.buffer.vertices.first() {
                first.corner_code.to_bits().hash(&mut hash);
                first.elevation.to_bits().hash(&mut hash);
                first.distance.to_bits().hash(&mut hash);
            }
        }
    }
    hash.finish()
}

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use super::{kind, prepare, SpatialBuffer, StructureKind};
use crate::{
    coords::WorldTileCoords,
    style::Style,
    tcs::{tiles::Tiles, world::World},
    terrain::{coverage::TerrainCoverageIndex, source::dem_source, DemTileComponent},
    vector::content,
};

#[derive(Default)]
struct CachedProfiles {
    key: u64,
    buffers: Arc<Vec<SpatialBuffer>>,
    style_rev: Option<u64>,
    style_digest: u64,
    has_markers: bool,
    has_non_overlay: bool,
    structure_layer_ids: rustc_hash::FxHashSet<String>,
}

fn digest_style_structures(style: &Style) -> (u64, bool, bool, rustc_hash::FxHashSet<String>) {
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
        structure_layer_ids,
    )
}

pub(crate) fn for_frame(
    world: &mut World,
    style: &Style,
    sources: &[WorldTileCoords],
) -> (Arc<Vec<SpatialBuffer>>, bool) {
    if dem_source(style).is_none() {
        if let Some(cached) = world.resources.get::<CachedProfiles>() {
            if cached.key == 0 {
                return (cached.buffers.clone(), false);
            }
        }
        let buffers = Arc::new(Vec::new());
        world.resources.insert(CachedProfiles {
            key: 0,
            buffers: buffers.clone(),
            ..CachedProfiles::default()
        });
        return (buffers, false);
    }
    let style_rev = style.revision();
    let cached_style = world
        .resources
        .get::<CachedProfiles>()
        .filter(|c| c.style_rev == Some(style_rev))
        .map(|c| {
            (
                c.style_digest,
                c.has_markers,
                c.has_non_overlay,
                c.structure_layer_ids.clone(),
            )
        });
    let (style_digest, has_markers, has_non_overlay, structure_layer_ids) =
        cached_style.unwrap_or_else(|| digest_style_structures(style));
    if !has_non_overlay {
        let buffers = world
            .resources
            .get::<CachedProfiles>()
            .filter(|cached| cached.key == 0)
            .map_or_else(|| Arc::new(Vec::new()), |cached| cached.buffers.clone());
        world.resources.insert(CachedProfiles {
            key: 0,
            buffers: buffers.clone(),
            style_rev: Some(style_rev),
            style_digest,
            has_markers,
            has_non_overlay,
            structure_layer_ids,
        });
        return (buffers, false);
    }
    // The terrain this frame draws, which markers stand on.
    let drawn = world.resources.get::<TerrainCoverageIndex>();
    let key = fingerprint(
        &world.tiles,
        sources,
        drawn,
        (style_digest, has_markers, &structure_layer_ids),
    );
    if let Some(cached) = world.resources.get::<CachedProfiles>() {
        if cached.key == key && cached.style_rev == Some(style_rev) {
            return (cached.buffers.clone(), false);
        }
    }
    let buffers = Arc::new(prepare(&world.tiles, style, sources, drawn));
    world.resources.insert(CachedProfiles {
        key,
        buffers: buffers.clone(),
        style_rev: Some(style_rev),
        style_digest,
        has_markers,
        has_non_overlay,
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
    // A marker moves when the drawn terrain switches DEM tiles, even if none has loaded. The
    // index is rebuilt every frame in no fixed order, so its tiles are sorted first.
    if let Some(drawn) = drawn.filter(|_| has_markers) {
        let mut terrain: Vec<_> = drawn
            .rendered_tiles()
            .map(|coords| (coords, drawn.loaded_dem_for(coords)))
            .collect();
        terrain.sort_unstable();
        terrain.hash(&mut hash);
    }
    for coords in tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
    {
        if let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) {
            coords.hash(&mut hash);
            dem.revision.hash(&mut hash);
            dem.tile.min().to_bits().hash(&mut hash);
            dem.tile.max().to_bits().hash(&mut hash);
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
            bucket.buffer.buffer.vertices.len().hash(&mut hash);
            if let Some(first) = bucket.buffer.buffer.vertices.first() {
                first.corner_code.to_bits().hash(&mut hash);
                first.elevation.to_bits().hash(&mut hash);
                first.distance.to_bits().hash(&mut hash);
            }
        }
    }
    hash.finish()
}

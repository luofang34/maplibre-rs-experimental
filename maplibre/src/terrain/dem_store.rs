//! Canonical per-source CPU elevation store shared by terrain meshes, hillshade, and colour relief.

use std::{collections::BTreeMap, sync::Arc};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::RasterSourceId,
    style::{source::Source, Style},
    tcs::tiles::{TileComponent, Tiles},
    terrain::{
        backfill::neighbours,
        dem::{self, DemTile},
        source::dem_source,
        DemTileComponent, LoadedDem,
    },
};

/// Decoded 2-texel bordered [`LoadedDem`] tiles keyed by `raster-dem` source identity.
#[derive(Default, Debug)]
pub struct DemLayersComponent {
    sources: BTreeMap<RasterSourceId, LoadedDem>,
}

impl TileComponent for DemLayersComponent {}

impl DemLayersComponent {
    /// Borrows the loaded elevation tile for `source`, if resident.
    #[inline]
    pub fn get(&self, source: &RasterSourceId) -> Option<&LoadedDem> {
        self.sources.get(source)
    }

    /// Mutably borrows the loaded elevation tile for `source`, if resident.
    #[inline]
    pub fn get_mut(&mut self, source: &RasterSourceId) -> Option<&mut LoadedDem> {
        self.sources.get_mut(source)
    }

    /// Whether `source` has a decoded elevation tile at these coordinates.
    #[inline]
    pub fn contains(&self, source: &RasterSourceId) -> bool {
        self.sources.contains_key(source)
    }

    /// Whether any `raster-dem` source has a decoded elevation tile at these coordinates.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Iterates over every resident `(source, LoadedDem)` pair on this tile.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&RasterSourceId, &LoadedDem)> {
        self.sources.iter()
    }

    /// Inserts or replaces the elevation tile for `source`.
    #[inline]
    pub fn insert(&mut self, source: RasterSourceId, dem: LoadedDem) {
        self.sources.insert(source, dem);
    }

    /// Removes the elevation tile of `source`, if present.
    #[inline]
    pub fn remove(&mut self, source: &RasterSourceId) -> Option<LoadedDem> {
        self.sources.remove(source)
    }
}

/// Returns the channel unpack vector `[r, g, b, base]` if `source` names a `raster-dem` source.
pub fn dem_unpack_for_source(style: &Style, source: &RasterSourceId) -> Option<[f64; 4]> {
    match source.name().and_then(|name| style.sources.get(name)) {
        Some(Source::RasterDem(dem)) => Some(dem.unpack_vector()),
        _ => None,
    }
}

/// Whether `source` is the active `style.terrain` elevation source.
pub fn is_terrain_source(style: &Style, source: &RasterSourceId) -> bool {
    dem_source(style).is_some_and(|dem| source.name() == Some(dem.name.as_str()))
}

/// Returns every `raster-dem` [`RasterSourceId`] in `style` with an identical [`Source::RasterDem`]
/// definition to `source` (sorted deterministically by source name), or `[source]` when unmatched.
pub fn equivalent_dem_sources(style: &Style, source: &RasterSourceId) -> Vec<RasterSourceId> {
    let Some(Source::RasterDem(target)) = source.name().and_then(|name| style.sources.get(name))
    else {
        return vec![source.clone()];
    };
    let mut names: Vec<&str> = style
        .sources
        .iter()
        .filter_map(|(name, candidate)| match candidate {
            Source::RasterDem(dem) if dem == target => Some(name.as_str()),
            _ => None,
        })
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        vec![source.clone()]
    } else {
        names.into_iter().map(RasterSourceId::from).collect()
    }
}

/// Returns the deterministic canonical [`RasterSourceId`] representing all identical `raster-dem`
/// sources in `style`, allowing [`GpuDemPool`](super::GpuDemPool) to share one GPU texture.
pub fn canonical_dem_source(style: &Style, source: &RasterSourceId) -> RasterSourceId {
    equivalent_dem_sources(style, source)
        .into_iter()
        .next()
        .unwrap_or_else(|| source.clone())
}

/// Decodes a raw `RgbaImage` into a despiked 2-texel bordered [`LoadedDem`].
pub fn decode_image_to_dem(
    coords: WorldTileCoords,
    image: &RgbaImage,
    unpack: [f64; 4],
    circumference_meters: f64,
) -> Option<LoadedDem> {
    let sample_metres = dem::sample_metres(coords, image.width(), circumference_meters);
    match DemTile::from_image_despiked(image, unpack, sample_metres) {
        Ok(tile) => Some(LoadedDem::new(tile)),
        Err(error) => {
            tracing::warn!(%coords, %error, "DEM tile image is unusable");
            None
        }
    }
}

/// Ingests `dem` under `(source, coords)`, backfills borders with all loaded neighbours of
/// `source`, and mirrors updates into [`DemTileComponent`] when `sync_terrain` is set.
pub fn ingest_source_dem(
    tiles: &mut Tiles,
    source: &RasterSourceId,
    coords: WorldTileCoords,
    dem: LoadedDem,
    sync_terrain: bool,
) {
    ensure_dem_layers(tiles, coords);
    if let Some(component) = tiles.query_mut::<&mut DemLayersComponent>(coords) {
        component.insert(source.clone(), dem);
    }
    let updated_neighbours = backfill_source_neighbours(tiles, source, coords);
    if sync_terrain {
        sync_coord_to_terrain_component(tiles, source, coords);
        for neighbour in updated_neighbours {
            sync_coord_to_terrain_component(tiles, source, neighbour);
        }
    }
}

/// Ingests `dem` for `source`, deduplicating CPU [`DemTile`] storage and border backfill across
/// all identical `raster-dem` sources in `style` (including `style.terrain` when equivalent).
pub fn ingest_source_dem_with_style(
    tiles: &mut Tiles,
    style: Option<&Style>,
    source: &RasterSourceId,
    coords: WorldTileCoords,
    dem: LoadedDem,
) {
    let Some(style) = style else {
        ingest_source_dem(tiles, source, coords, dem, false);
        return;
    };
    let equiv = equivalent_dem_sources(style, source);
    let canonical = equiv.first().cloned().unwrap_or_else(|| source.clone());
    let sync_terrain = equiv.iter().any(|eq| is_terrain_source(style, eq));
    if sync_terrain
        && matches!(
            tiles.query::<&DemTileComponent>(coords),
            Some(DemTileComponent::Loaded(_))
        )
    {
        sync_terrain_dem_to_store(tiles, style, coords);
        return;
    }
    for at in std::iter::once(coords).chain(neighbours(coords).into_iter().map(|(n, _)| n)) {
        if let Some(layers) = tiles.query_mut::<&mut DemLayersComponent>(at) {
            for sibling in equiv.iter().skip(1) {
                layers.remove(sibling);
            }
        }
    }
    ensure_dem_layers(tiles, coords);
    if let Some(component) = tiles.query_mut::<&mut DemLayersComponent>(coords) {
        component.insert(canonical.clone(), dem);
    }
    let updated_neighbours = backfill_source_neighbours(tiles, &canonical, coords);
    for at in std::iter::once(coords).chain(updated_neighbours.iter().copied()) {
        mirror_canonical_to_siblings(tiles, &canonical, &equiv[1..], at);
        if sync_terrain {
            sync_coord_to_terrain_component(tiles, &canonical, at);
        }
    }
    for (neighbour, _) in neighbours(coords) {
        mirror_canonical_to_siblings(tiles, &canonical, &equiv[1..], neighbour);
    }
}

/// Releases [`DemLayersComponent`] mirrors of the active terrain DEM source on `coords` and its
/// 8 neighbours before `backfill_neighbours` runs so `Arc::make_mut` mutates [`DemTile`] in place.
pub fn release_terrain_store_mirrors(tiles: &mut Tiles, style: &Style, coords: WorldTileCoords) {
    let Some(dem_spec) = dem_source(style) else {
        return;
    };
    let source = RasterSourceId::from(dem_spec.name.as_str());
    let equiv = equivalent_dem_sources(style, &source);
    for at in std::iter::once(coords).chain(neighbours(coords).into_iter().map(|(n, _)| n)) {
        if let Some(layers) = tiles.query_mut::<&mut DemLayersComponent>(at) {
            for eq in &equiv {
                layers.remove(eq);
            }
        }
    }
}

/// Mirrors a freshly ingested & backfilled [`DemTileComponent`] (and its neighbours) into
/// [`DemLayersComponent`] under every equivalent `raster-dem` source so `hillshade` and
/// `color-relief` reuse the decoded [`LoadedDem`] without fetching or decoding it again.
pub fn sync_terrain_dem_to_store(tiles: &mut Tiles, style: &Style, coords: WorldTileCoords) {
    let Some(dem_spec) = dem_source(style) else {
        return;
    };
    let source = RasterSourceId::from(dem_spec.name.as_str());
    let equiv = equivalent_dem_sources(style, &source);
    sync_terrain_coord_to_layers(tiles, &equiv, coords);
    for (neighbour, _) in neighbours(coords) {
        sync_terrain_coord_to_layers(tiles, &equiv, neighbour);
    }
}

fn ensure_dem_layers(tiles: &mut Tiles, coords: WorldTileCoords) {
    if tiles.query::<&DemLayersComponent>(coords).is_none() {
        if let Some(mut tile) = tiles.spawn_mut(coords) {
            tile.insert(DemLayersComponent::default());
        }
    }
}

fn mirror_canonical_to_siblings(
    tiles: &mut Tiles,
    canonical: &RasterSourceId,
    siblings: &[RasterSourceId],
    coords: WorldTileCoords,
) {
    if siblings.is_empty() {
        return;
    }
    let Some(loaded) = tiles
        .query::<&DemLayersComponent>(coords)
        .and_then(|layers| layers.get(canonical))
        .cloned()
    else {
        return;
    };
    if let Some(layers) = tiles.query_mut::<&mut DemLayersComponent>(coords) {
        for sibling in siblings {
            layers.insert(sibling.clone(), loaded.clone());
        }
    }
}

fn sync_coord_to_terrain_component(
    tiles: &mut Tiles,
    source: &RasterSourceId,
    coords: WorldTileCoords,
) {
    let Some(loaded) = tiles
        .query::<&DemLayersComponent>(coords)
        .and_then(|layers| layers.get(source))
        .cloned()
    else {
        return;
    };
    if let Some(mut tile) = tiles.spawn_mut(coords) {
        tile.insert(DemTileComponent::Loaded(loaded));
    }
}

fn sync_terrain_coord_to_layers(
    tiles: &mut Tiles,
    sources: &[RasterSourceId],
    coords: WorldTileCoords,
) {
    let Some(DemTileComponent::Loaded(loaded)) = tiles.query::<&DemTileComponent>(coords).cloned()
    else {
        return;
    };
    ensure_dem_layers(tiles, coords);
    if let Some(layers) = tiles.query_mut::<&mut DemLayersComponent>(coords) {
        for source in sources {
            layers.insert(source.clone(), loaded.clone());
        }
    }
}

/// Backfills the 2-pixel borders between `coords` and every loaded neighbour of `source`,
/// returning the neighbour coordinates whose border changed.
pub fn backfill_source_neighbours(
    tiles: &mut Tiles,
    source: &RasterSourceId,
    coords: WorldTileCoords,
) -> Vec<WorldTileCoords> {
    let mut updated = Vec::new();
    for (neighbour, (dx, dy)) in neighbours(coords) {
        let _ = fill_source_border(tiles, source, coords, neighbour, dx, dy);
        if fill_source_border(tiles, source, neighbour, coords, -dx, -dy) {
            updated.push(neighbour);
        }
    }
    updated
}

fn fill_source_border(
    tiles: &mut Tiles,
    source_id: &RasterSourceId,
    target: WorldTileCoords,
    source: WorldTileCoords,
    dx: i32,
    dy: i32,
) -> bool {
    let Some(source_dem) = tiles
        .query::<&DemLayersComponent>(source)
        .and_then(|layers| layers.get(source_id))
    else {
        return false;
    };
    let samples = match source_dem.tile.edge_samples(dx, dy) {
        Ok(samples) => samples,
        Err(error) => {
            tracing::warn!(%target, %source, %error, "cannot read DEM neighbour edge");
            return false;
        }
    };
    let source_dim = source_dem.tile.dim();
    let source_unpack = source_dem.tile.unpack();
    let Some(target_dem) = tiles
        .query_mut::<&mut DemLayersComponent>(target)
        .and_then(|layers| layers.get_mut(source_id))
    else {
        return false;
    };
    let result = target_dem
        .tile
        .validate_neighbour(source_dim, source_unpack)
        .and_then(|()| {
            if target_dem.tile.border_matches(dx, dy, &samples)? {
                Ok(false)
            } else {
                Arc::make_mut(&mut target_dem.tile).fill_border(dx, dy, &samples)
            }
        });
    match result {
        Ok(true) => {
            target_dem.revision = target_dem.revision.wrapping_add(1);
            true
        }
        Ok(false) => false,
        Err(error) => {
            tracing::warn!(%target, %source, %error, "cannot backfill DEM border");
            false
        }
    }
}

//! How each tile's requests stand, as GL JS `Tile.state`, and which tiles are loading.
//!
//! A request family marks a tile loading when it admits a request for it and settled when the
//! request's outcome lands or the request is given up. The tiles marked loading are kept in a
//! set, so counting them reads that set instead of every tile.

use rustc_hash::FxHashSet as HashSet;

pub use crate::io::frame_signals::RequestKind;
use crate::{
    coords::WorldTileCoords,
    tcs::{
        tiles::{TileComponent, Tiles},
        world::World,
    },
};

/// Where one request family's request for a tile stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LoadState {
    /// Nothing was requested yet, or the request was given up before it brought anything.
    #[default]
    Unloaded,
    /// The first request is in flight.
    Loading,
    /// A request is in flight for a tile that already shows an earlier result.
    Reloading,
    /// The last request brought its result.
    Loaded,
    /// The last request failed temporarily and is scheduled for retry later.
    Errored,
}

impl LoadState {
    fn is_loading(self) -> bool {
        matches!(self, Self::Loading | Self::Reloading)
    }

    fn on_issued(self) -> Self {
        match self {
            Self::Loaded | Self::Reloading => Self::Reloading,
            Self::Unloaded | Self::Loading | Self::Errored => Self::Loading,
        }
    }

    fn on_landed(errored: bool) -> Self {
        if errored {
            Self::Errored
        } else {
            Self::Loaded
        }
    }

    fn on_abandoned(self) -> Self {
        match self {
            Self::Reloading => Self::Loaded,
            Self::Loading => Self::Unloaded,
            settled => settled,
        }
    }
}

/// A tile's load state per request family.
#[derive(Debug, Default)]
pub(crate) struct TileLoad {
    raster: LoadState,
    dem: LoadState,
    vector: LoadState,
}
impl TileComponent for TileLoad {}

impl TileLoad {
    fn slot(&self, kind: RequestKind) -> LoadState {
        match kind {
            RequestKind::Raster => self.raster,
            RequestKind::Dem => self.dem,
            RequestKind::Vector => self.vector,
        }
    }

    fn slot_mut(&mut self, kind: RequestKind) -> &mut LoadState {
        match kind {
            RequestKind::Raster => &mut self.raster,
            RequestKind::Dem => &mut self.dem,
            RequestKind::Vector => &mut self.vector,
        }
    }

    /// The state of the family's request.
    pub(crate) fn state(&self, kind: RequestKind) -> LoadState {
        self.slot(kind)
    }

    /// Whether some family's request is in flight.
    pub(crate) fn is_loading(&self) -> bool {
        [self.raster, self.dem, self.vector]
            .into_iter()
            .any(LoadState::is_loading)
    }
}

/// Tiles marked loading since they were last counted; a tile that settled or was evicted since
/// leaves the set the next time it is pruned.
#[derive(Debug, Default)]
struct LoadingTiles {
    tiles: HashSet<WorldTileCoords>,
    last_pruned_frame: u64,
}

/// Cumulative count of tile requests admitted across all families in this world.
#[derive(Clone, Copy, Debug, Default)]
struct TotalRequestsIssued(u64);

fn ensure_tile_load(world: &mut World, coords: WorldTileCoords) -> Option<&mut TileLoad> {
    if world.tiles.query::<&TileLoad>(coords).is_none() {
        let mut tile = world.tiles.spawn_mut(coords)?;
        tile.insert(TileLoad::default());
    }
    world.tiles.query_mut::<&mut TileLoad>(coords)
}

fn record_loading_tile(world: &mut World, coords: WorldTileCoords) {
    world
        .resources
        .get_or_init_mut::<LoadingTiles>()
        .tiles
        .insert(coords);
    let total = world.resources.get_or_init_mut::<TotalRequestsIssued>();
    total.0 = total.0.saturating_add(1);
}

fn collect_settled_tiles(world: &World, loading: &LoadingTiles) -> Vec<WorldTileCoords> {
    loading
        .tiles
        .iter()
        .filter(|coords| !is_loading(&world.tiles, **coords))
        .copied()
        .collect()
}

/// Marks the family's request for the tile in flight.
pub(crate) fn issued(world: &mut World, coords: WorldTileCoords, kind: RequestKind) {
    let Some(load) = ensure_tile_load(world, coords) else {
        return;
    };
    let state = load.slot_mut(kind);
    *state = state.on_issued();
    prune(world);
    record_loading_tile(world, coords);
}

/// Settles the family's request for the tile: loaded, or errored when it is retried later.
pub(crate) fn landed(world: &mut World, coords: WorldTileCoords, kind: RequestKind, errored: bool) {
    set(world, coords, kind, LoadState::on_landed(errored));
}

/// Settles a request given up before its outcome: the tile keeps what it showed before.
pub(crate) fn abandoned(world: &mut World, coords: WorldTileCoords, kind: RequestKind) {
    let Some(load) = world.tiles.query::<&TileLoad>(coords) else {
        return;
    };
    let settled = load.state(kind).on_abandoned();
    set(world, coords, kind, settled);
}

fn set(world: &mut World, coords: WorldTileCoords, kind: RequestKind, state: LoadState) {
    if let Some(load) = world.tiles.query_mut::<&mut TileLoad>(coords) {
        *load.slot_mut(kind) = state;
    }
}

/// Whether some request for the tile is in flight.
pub(crate) fn is_loading(tiles: &Tiles, coords: WorldTileCoords) -> bool {
    tiles
        .query::<&TileLoad>(coords)
        .is_some_and(TileLoad::is_loading)
}

/// Tiles with a request in flight, read from the tiles marked loading.
pub(crate) fn loading_count(world: &World) -> usize {
    world.resources.get::<LoadingTiles>().map_or(0, |loading| {
        loading
            .tiles
            .iter()
            .filter(|coords| is_loading(&world.tiles, **coords))
            .count()
    })
}

/// Cumulative count of tile requests admitted across all families in `world`.
pub(crate) fn total_issued(world: &World) -> u64 {
    world
        .resources
        .get::<TotalRequestsIssued>()
        .map_or(0, |total| total.0)
}

/// Drops the tiles that settled or were evicted since they were marked loading.
fn prune(world: &mut World) {
    let host_frame = crate::io::frame_signals::host_frame_in_progress(world);
    let Some(loading) = world.resources.get::<LoadingTiles>() else {
        return;
    };
    if host_frame > 0 && loading.last_pruned_frame == host_frame {
        return;
    }
    let settled = collect_settled_tiles(world, loading);
    if let Some(loading) = world.resources.get_mut::<LoadingTiles>() {
        loading.last_pruned_frame = host_frame;
        for coords in settled {
            loading.tiles.remove(&coords);
        }
    }
}

#[cfg(test)]
mod tests;

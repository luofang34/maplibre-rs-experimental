//! How each tile's requests stand, as GL JS `Tile.state`, and which tiles are loading.
//!
//! A request family marks a tile loading when it admits a request for it and settled when the
//! request's outcome lands or the request is given up. The tiles marked loading are kept in a
//! set, so counting them reads that set instead of every tile.

use std::collections::HashSet;

use crate::{
    coords::WorldTileCoords,
    io::tile_retry::RequestKind,
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
    /// The last request failed for now and is retried later.
    Errored,
}

impl LoadState {
    fn is_loading(self) -> bool {
        matches!(self, Self::Loading | Self::Reloading)
    }
}

/// A tile's load state per request family.
#[derive(Debug, Default)]
pub(crate) struct TileLoad([LoadState; 3]);
impl TileComponent for TileLoad {}

impl TileLoad {
    /// The state of the family's request.
    pub(crate) fn state(&self, kind: RequestKind) -> LoadState {
        self.0[kind.index()]
    }

    /// Whether some family's request is in flight.
    pub(crate) fn is_loading(&self) -> bool {
        self.0.iter().any(|state| state.is_loading())
    }
}

/// Tiles marked loading since they were last counted; a tile that settled or was evicted since
/// leaves the set the next time it is pruned.
#[derive(Debug, Default)]
struct LoadingTiles(HashSet<WorldTileCoords>);

/// Marks the family's request for the tile in flight.
pub(crate) fn issued(world: &mut World, coords: WorldTileCoords, kind: RequestKind) {
    if world.tiles.query::<&TileLoad>(coords).is_none() {
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return;
        };
        tile.insert(TileLoad::default());
    }
    if let Some(load) = world.tiles.query_mut::<&mut TileLoad>(coords) {
        let state = &mut load.0[kind.index()];
        *state = match *state {
            LoadState::Loaded | LoadState::Reloading => LoadState::Reloading,
            LoadState::Unloaded | LoadState::Loading | LoadState::Errored => LoadState::Loading,
        };
    }
    prune(world);
    world
        .resources
        .get_or_init_mut::<LoadingTiles>()
        .0
        .insert(coords);
}

/// Settles the family's request for the tile: loaded, or errored when it is retried later.
pub(crate) fn landed(world: &mut World, coords: WorldTileCoords, kind: RequestKind, errored: bool) {
    set(
        world,
        coords,
        kind,
        if errored {
            LoadState::Errored
        } else {
            LoadState::Loaded
        },
    );
}

/// Settles a request given up before its outcome: the tile keeps what it showed before.
pub(crate) fn abandoned(world: &mut World, coords: WorldTileCoords, kind: RequestKind) {
    let Some(load) = world.tiles.query::<&TileLoad>(coords) else {
        return;
    };
    let settled = match load.state(kind) {
        LoadState::Reloading => LoadState::Loaded,
        LoadState::Loading => LoadState::Unloaded,
        settled => settled,
    };
    set(world, coords, kind, settled);
}

fn set(world: &mut World, coords: WorldTileCoords, kind: RequestKind, state: LoadState) {
    if let Some(load) = world.tiles.query_mut::<&mut TileLoad>(coords) {
        load.0[kind.index()] = state;
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
            .0
            .iter()
            .filter(|coords| is_loading(&world.tiles, **coords))
            .count()
    })
}

/// Drops the tiles that settled or were evicted since they were marked loading.
fn prune(world: &mut World) {
    let Some(loading) = world.resources.get::<LoadingTiles>() else {
        return;
    };
    let settled: Vec<_> = loading
        .0
        .iter()
        .filter(|coords| !is_loading(&world.tiles, **coords))
        .copied()
        .collect();
    if let Some(loading) = world.resources.get_mut::<LoadingTiles>() {
        for coords in settled {
            loading.0.remove(&coords);
        }
    }
}

#[cfg(test)]
mod tests;

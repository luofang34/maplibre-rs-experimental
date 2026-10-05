//! The tiles whose labels name provided images: which names they drew, at what pixel ratio,
//! and whether their worker still waits for some of them.
//!
//! A tile's worker lays its labels out at once with what is ready and keeps waiting for the
//! rest; the map keeps drawing frames while it does, so the labels it sends again when the
//! images arrive reach the screen of an otherwise idle map. A tile that leaves the view stops
//! its waiting worker. A tile is requested again only when its images must be made anew: its
//! namespace was invalidated, the display's pixel ratio changed, or a provider was
//! temporarily unavailable.

use std::{collections::HashSet, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{IntoMessage, Message, MessageTag},
        tile_retry::{self, RequestKind},
    },
    tcs::{tiles::TileComponent, world::World},
};

/// The wait before a tile whose provider was unavailable is requested again, doubling with
/// each unavailable request in a row up to [`RETRY_LAST`].
const RETRY_FIRST: Duration = Duration::from_secs(1);
/// The longest wait between requests of a tile whose provider stays unavailable.
const RETRY_LAST: Duration = Duration::from_secs(60);

/// Where a tile's worker is with the provided images its labels name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvidedImagesState {
    /// Labels were sent with the images that were ready; the worker waits for the rest.
    Awaiting,
    /// Every image has an answer, and labels that gained one were sent again.
    Settled,
    /// A provider was unavailable for now; the tile is requested again after a back-off.
    Retry,
}

/// A worker's report on the provided images one tile's labels name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProvidedImagesReport {
    /// The tile.
    pub coords: WorldTileCoords,
    /// The request the report belongs to.
    pub attempt: Option<u64>,
    /// The provided names the tile's labels ask for.
    pub names: Vec<String>,
    /// The pixel ratio the images were asked for.
    pub pixel_ratio: f32,
    /// Where the worker is.
    pub state: ProvidedImagesState,
}

/// Routes [`ProvidedImagesReport`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProvidedImagesTag;

impl MessageTag for ProvidedImagesTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

impl ProvidedImagesReport {
    /// The tag reports travel under.
    pub fn message_tag() -> &'static dyn MessageTag {
        &ProvidedImagesTag
    }
}

impl IntoMessage for ProvidedImagesReport {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

/// Device pixels per layout pixel of the display, as the host reported it. Without it the
/// ratio is read from the surface and viewport sizes, which rounding makes slightly off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DisplayPixelRatio(pub(crate) f32);

/// Records the display's pixel ratio, which provided images are made for.
pub(crate) fn set_pixel_ratio(world: &mut World, ratio: f64) {
    world.resources.insert(DisplayPixelRatio(ratio as f32));
}

/// The provided images of one tile's labels.
#[derive(Default)]
pub(crate) struct TileProvidedImages {
    names: Vec<String>,
    pixel_ratio: f32,
    /// The request whose worker still waits for images.
    awaiting: Option<u64>,
    /// Requests in a row that found a provider unavailable, which the back-off doubles with.
    unavailable: u32,
}

impl TileComponent for TileProvidedImages {}

/// Applies a worker's report, unless a newer request for the tile replaced the one it is from.
pub(crate) fn apply(world: &mut World, report: ProvidedImagesReport) {
    let coords = report.coords;
    if !tile_retry::accepts(world, coords, RequestKind::Vector, report.attempt) {
        return;
    }
    if world.tiles.query::<&TileProvidedImages>(coords).is_none() {
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return;
        };
        tile.insert(TileProvidedImages::default());
    }
    let mut retry = None;
    if let Some(tile) = world.tiles.query_mut::<&mut TileProvidedImages>(coords) {
        tile.names = report.names;
        tile.pixel_ratio = report.pixel_ratio;
        tile.awaiting = (report.state == ProvidedImagesState::Awaiting)
            .then_some(report.attempt)
            .flatten();
        match report.state {
            ProvidedImagesState::Awaiting => {}
            ProvidedImagesState::Settled => tile.unavailable = 0,
            ProvidedImagesState::Retry => {
                retry = Some(RETRY_FIRST.saturating_mul(1 << tile.unavailable.min(6)));
                tile.unavailable = tile.unavailable.saturating_add(1);
            }
        }
    }
    if let Some(delay) = retry {
        tile_retry::retry_later(world, coords, RequestKind::Vector, delay.min(RETRY_LAST));
    }
    crate::render::frame_signals::mark_dirty(world);
}

/// Whether a tile's worker still waits for images, so frames must keep coming to show them.
pub(crate) fn awaiting(world: &World) -> bool {
    world.tiles.tiles.values().any(|tile| {
        world
            .tiles
            .query::<&TileProvidedImages>(tile.coords)
            .is_some_and(|images| images.awaiting.is_some())
    })
}

/// Whether the tile drew provided images for another pixel ratio than `pixel_ratio`.
pub(crate) fn drawn_for_another_ratio(
    world: &World,
    coords: WorldTileCoords,
    pixel_ratio: f32,
) -> bool {
    world
        .tiles
        .query::<&TileProvidedImages>(coords)
        .is_some_and(|images| !images.names.is_empty() && images.pixel_ratio != pixel_ratio)
}

/// Notes that a tile was requested anew for `pixel_ratio`, so it is not requested again for
/// that ratio before its worker reports, and forgets its wait, returning the attempt whose
/// worker waited: the new request reports its own images, and the old one's labels would be
/// refused.
pub(crate) fn restarted(
    world: &mut World,
    coords: WorldTileCoords,
    pixel_ratio: f32,
) -> Option<u64> {
    let images = world.tiles.query_mut::<&mut TileProvidedImages>(coords)?;
    images.pixel_ratio = pixel_ratio;
    // The new request reports the names it draws; a style that dropped them leaves none.
    images.names.clear();
    images.awaiting.take()
}

/// Stops the waiting workers of tiles no longer wanted, returning their attempts.
pub(crate) fn release_unwanted(world: &mut World, wanted: &HashSet<WorldTileCoords>) -> Vec<u64> {
    let coords: Vec<WorldTileCoords> = world
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| !wanted.contains(coords))
        .collect();
    let mut released = Vec::new();
    for coords in coords {
        if let Some(images) = world.tiles.query_mut::<&mut TileProvidedImages>(coords) {
            released.extend(images.awaiting.take());
        }
    }
    released
}

/// Forgets the answers of `namespace` in `providers` and requests again the tiles that drew
/// them, returning how many: what a map does when a host invalidates a namespace.
pub(crate) fn invalidate_namespace(
    providers: Option<&crate::sdf::assets::ImageProviders>,
    world: &mut World,
    namespace: &str,
) -> usize {
    if let Some(providers) = providers {
        providers.invalidate(namespace);
    }
    invalidate(world, namespace)
}

/// Requests again the tiles whose labels name an image of `namespace`, returning how many.
pub(crate) fn invalidate(world: &mut World, namespace: &str) -> usize {
    let prefix = format!("{namespace}:");
    let tiles: Vec<WorldTileCoords> = world
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| {
            world
                .tiles
                .query::<&TileProvidedImages>(*coords)
                .is_some_and(|images| images.names.iter().any(|name| name.starts_with(&prefix)))
        })
        .collect();
    tile_retry::refresh_tiles(world, RequestKind::Vector, &tiles);
    tiles.len()
}

#[cfg(test)]
mod tests;

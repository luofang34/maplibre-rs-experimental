//! Reads vector tile bytes compiled into the binary.

use std::{concat, env};

#[cfg(static_tiles_found)]
use include_dir::include_dir;
use include_dir::Dir;
use thiserror::Error;

use crate::coords::TileCoords;

#[cfg(static_tiles_found)]
static TILES: Dir = include_dir!("$OUT_DIR/extracted-tiles");
#[cfg(not(static_tiles_found))]
static TILES: Dir = Dir::new("extracted-tiles", &[]);

/// A tile could not be read from the binary's embedded archive.
#[derive(Debug, Error)]
pub enum StaticFetchError {
    /// No tiles were compiled into this binary.
    #[error("no tiles are embedded in this binary; requested {}/{}/{}", .coords.z, .coords.x, .coords.y)]
    EmptyArchive {
        /// Requested tile location.
        coords: TileCoords,
    },
    /// The archive exists but has no entry for the requested tile.
    #[error("tile {}/{}/{} is not embedded", .coords.z, .coords.x, .coords.y)]
    NotFound {
        /// Requested tile location.
        coords: TileCoords,
    },
}

/// Fetches owned copies of PBF tiles embedded by the build script.
///
/// Requires `embed-static-tiles`. If the build has no tile archive, construction still succeeds
/// and fetches return [`StaticFetchError::EmptyArchive`]. No runtime filesystem access is needed.
#[derive(Default)]
pub struct StaticTileFetcher;

impl StaticTileFetcher {
    /// Build-time directory used to embed tiles; it need not exist on the running machine.
    pub fn get_source_path() -> &'static str {
        concat!(env!("OUT_DIR"), "/extracted-tiles")
    }

    /// Creates a fetcher without inspecting the filesystem or checking tile availability.
    pub fn new() -> Self {
        Self {}
    }

    /// Copies an embedded tile using an async-compatible interface; performs no asynchronous I/O.
    /// Returns the same archive and lookup errors as [`Self::sync_fetch_tile`].
    pub async fn fetch_tile(&self, coords: &TileCoords) -> Result<Vec<u8>, StaticFetchError> {
        self.sync_fetch_tile(coords)
    }

    /// Copies the `{z}/{x}/{y}.pbf` entry from memory without changing coordinate addressing.
    /// Returns a typed error identifying the tile if the archive or entry is missing.
    pub fn sync_fetch_tile(&self, coords: &TileCoords) -> Result<Vec<u8>, StaticFetchError> {
        if TILES.entries().is_empty() {
            return Err(StaticFetchError::EmptyArchive { coords: *coords });
        }

        let tile = TILES
            .get_file(format!("{}/{}/{}.pbf", coords.z, coords.x, coords.y))
            .ok_or(StaticFetchError::NotFound { coords: *coords })?;
        Ok(Vec::from(tile.contents()))
    }
}

#[cfg(test)]
mod tests;

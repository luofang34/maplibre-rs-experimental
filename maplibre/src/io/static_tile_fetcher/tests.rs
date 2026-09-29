#![allow(clippy::expect_used, clippy::panic)]

use super::StaticTileFetcher;
use crate::coords::{TileCoords, ZoomLevel};

fn absent_tile() -> TileCoords {
    (0, 0, ZoomLevel::new(0)).into()
}

#[test]
fn missing_tile_returns_an_error_with_its_coordinates() {
    let error = StaticTileFetcher::new()
        .sync_fetch_tile(&absent_tile())
        .expect_err("no world overview tile");
    assert!(
        error.to_string().contains("0/0/0"),
        "error must identify the requested tile: {error}"
    );
}

#[tokio::test]
async fn async_fetch_reports_an_absent_tile_without_panicking() {
    assert!(StaticTileFetcher::new()
        .fetch_tile(&absent_tile())
        .await
        .is_err());
}

#[cfg(static_tiles_found)]
#[tokio::test]
async fn embedded_tile_has_identical_bytes_in_both_fetch_interfaces() {
    let coords = (17425, 11365, ZoomLevel::new(15)).into();
    let fetcher = StaticTileFetcher::new();
    let sync = fetcher
        .sync_fetch_tile(&coords)
        .expect("embedded Munich tile");
    let asynchronous = fetcher
        .fetch_tile(&coords)
        .await
        .expect("embedded Munich tile");
    assert!(!sync.is_empty());
    assert_eq!(sync, asynchronous);
}

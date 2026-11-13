use super::{mesh_key, TileMeshUsage};
use crate::coords::{WorldTileCoords, ZoomLevel};

fn coords(x: i32, y: i32, zoom: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(zoom),
    }
}

#[test]
fn raster_granularity_matches_gl_js_policy() {
    assert_eq!(
        mesh_key(coords(0, 0, 0), TileMeshUsage::Raster, false).granularity,
        128
    );
    assert_eq!(
        mesh_key(coords(1, 1, 1), TileMeshUsage::Raster, false).granularity,
        64
    );
    assert_eq!(
        mesh_key(coords(4, 4, 4), TileMeshUsage::Raster, false).granularity,
        32
    );
    assert_eq!(
        mesh_key(coords(4, 4, 24), TileMeshUsage::Raster, false).granularity,
        32
    );
}

#[test]
fn stencil_granularity_reaches_one() {
    assert_eq!(
        mesh_key(coords(0, 0, 0), TileMeshUsage::Stencil, false).granularity,
        128
    );
    assert_eq!(
        mesh_key(coords(1, 1, 6), TileMeshUsage::Stencil, false).granularity,
        2
    );
    assert_eq!(
        mesh_key(coords(1, 1, 7), TileMeshUsage::Stencil, false).granularity,
        1
    );
    assert_eq!(
        mesh_key(coords(1, 1, 31), TileMeshUsage::Stencil, false).granularity,
        1
    );
}

#[test]
fn pole_extensions_follow_first_and_last_rows() {
    let zoom_zero = mesh_key(coords(0, 0, 0), TileMeshUsage::Raster, true);
    assert!(zoom_zero.extend_to_north_pole);
    assert!(zoom_zero.extend_to_south_pole);
    assert!(zoom_zero.generate_borders);

    let north = mesh_key(coords(0, 0, 2), TileMeshUsage::Raster, false);
    assert!(north.extend_to_north_pole);
    assert!(!north.extend_to_south_pole);

    let south = mesh_key(coords(0, 3, 2), TileMeshUsage::Raster, false);
    assert!(!south.extend_to_north_pole);
    assert!(south.extend_to_south_pole);
}

#[test]
fn globe_tile_mesh_cache_scratch_is_available() {
    use super::GlobeTileMeshCache;

    let mut cache = GlobeTileMeshCache::new();
    assert_eq!(cache.scratch().vertices.len(), 0);
    assert_eq!(cache.scratch().indices_u16.len(), 0);

    let flat_key = cache.key_for(coords(0, 0, 0), TileMeshUsage::Raster, true);
    assert_eq!(flat_key.granularity, 1);
    assert!(!flat_key.extend_to_north_pole);
    assert!(!flat_key.extend_to_south_pole);
    assert!(flat_key.generate_borders);

    cache.set_globe_active(true);
    let globe_key = cache.key_for(coords(0, 0, 0), TileMeshUsage::Raster, true);
    assert_eq!(globe_key.granularity, 128);
    assert!(globe_key.extend_to_north_pole);
    assert!(globe_key.extend_to_south_pole);
}

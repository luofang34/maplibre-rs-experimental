#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::{IndexedTileElevation, TerrainCoverageIndex, TerrainSample};
use crate::{
    coords::{TileCoords, WorldTileCoords, ZoomLevel},
    io::source_type::{RasterSource, SourceType},
    projection::globe::covering::{TileElevationProvider, TileElevationRange},
    style::source::TileAddressingScheme,
    tcs::tiles::Tiles,
    terrain::{dem::DemTile, source::DemSource, DemTileComponent, LoadedDem},
};

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

fn source(exaggeration: f32) -> DemSource {
    DemSource {
        name: "dem".to_string(),
        source: SourceType::Raster(RasterSource::from_template(
            "https://dem.example/{z}/{x}/{y}.png",
            TileAddressingScheme::XYZ,
        )),
        tile_size: 256,
        unpack: TERRARIUM,
        minzoom: 0,
        maxzoom: 12,
        exaggeration,
    }
}

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn terrarium_pixel(elevation: f64) -> Rgba<u8> {
    let value = elevation + 32768.0;
    let red = (value / 256.0).floor();
    let green = value - red * 256.0;
    Rgba([red as u8, green as u8, 0, 255])
}

/// A 4x4 tile whose samples read `base + 100 * x + 10 * y`.
fn gradient_tile(base: f64) -> DemTile {
    let mut image = RgbaImage::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            image.put_pixel(
                x,
                y,
                terrarium_pixel(base + 100.0 * f64::from(x) + 10.0 * f64::from(y)),
            );
        }
    }
    DemTile::from_image(&image, TERRARIUM).expect("tile decodes")
}

fn load(tiles: &mut Tiles, coords: WorldTileCoords, dem: DemTile) {
    tiles
        .spawn_mut(coords)
        .expect("valid coords")
        .insert(DemTileComponent::Loaded(LoadedDem::new(dem)));
}

#[test]
fn indexes_rendered_tiles_against_their_dem_and_tracks_padded_bounds() {
    let mut tiles = Tiles::default();
    // Rendered z5 tiles (6, 4) and (7, 4) both sample the z4 DEM tile (3, 2).
    load(&mut tiles, tile(3, 2, 4), gradient_tile(1000.0));

    let index = TerrainCoverageIndex::build(
        [tile(6, 4, 5), tile(7, 4, 5), tile(0, 0, 5)],
        &tiles,
        &source(2.0),
    );

    assert_eq!(index.loaded_dem_for(tile(6, 4, 5)), Some(tile(3, 2, 4)));
    assert_eq!(index.loaded_dem_for(tile(0, 0, 5)), None);
    // Samples span 1000..1330 metres, doubled by the exaggeration, plus ten metres of padding.
    assert!(
        (index.min_elevation() - -10.0).abs() < 1e-9,
        "sea level stays inside the bracket"
    );
    assert!((index.max_elevation() - 2670.0).abs() < 1e-9);
}

#[test]
fn samples_the_correct_part_of_a_parent_dem_tile() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(0, 0, 1), gradient_tile(0.0));
    // The rendered z2 tile (1, 1) is the bottom-right child of the z1 DEM tile (0, 0).
    let index = TerrainCoverageIndex::build([tile(1, 1, 2)], &tiles, &source(1.0));

    // The centre of that child sits at 75% of both parent axes: sample (3, 3) of a 4x4 grid.
    let sample = index.sample(&tiles, 0.375, 0.375);

    assert_eq!(
        sample,
        TerrainSample {
            covered: true,
            dem_loaded: true,
            elevation: 330.0,
        }
    );
}

#[test]
fn reports_coverage_without_elevation_while_the_dem_loads() {
    let mut tiles = Tiles::default();
    tiles
        .spawn_mut(tile(3, 2, 4))
        .expect("valid coords")
        .insert(DemTileComponent::Pending);
    let index = TerrainCoverageIndex::build([tile(6, 4, 5)], &tiles, &source(1.0));

    let inside = index.sample(&tiles, 6.5 / 32.0, 4.5 / 32.0);
    let outside = index.sample(&tiles, 0.5, 0.5);

    assert_eq!(
        inside,
        TerrainSample {
            covered: true,
            dem_loaded: false,
            elevation: 0.0,
        }
    );
    assert!(!outside.covered);
    assert_eq!(index.elevation_at(&tiles, 6.5 / 32.0, 4.5 / 32.0), None);
}

#[test]
fn prefers_the_finest_rendered_zoom_and_wraps_longitude() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(0, 0, 0), gradient_tile(0.0));
    load(&mut tiles, tile(0, 0, 3), gradient_tile(5000.0));
    let index = TerrainCoverageIndex::build([tile(0, 0, 1), tile(0, 0, 4)], &tiles, &source(1.0));

    let fine = index.sample(&tiles, 0.01, 0.01);
    let coarse = index.sample(&tiles, 0.4, 0.4);
    let wrapped = index.sample(&tiles, 1.01, 0.01);

    assert!(fine.elevation >= 5000.0, "finest rendered tile wins");
    assert!(
        coarse.elevation < 1000.0,
        "outside the fine tile the coarse tile answers"
    );
    assert_eq!(wrapped, fine);
}

#[test]
fn tile_elevation_range_uses_the_loaded_dem_or_the_fallback() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(3, 2, 4), gradient_tile(1000.0));
    let index = TerrainCoverageIndex::build([tile(6, 4, 5)], &tiles, &source(2.0));
    let fallback = TileElevationRange {
        min_meters: -500.0,
        max_meters: 9000.0,
    };
    let provider = IndexedTileElevation {
        index: &index,
        fallback,
    };

    let known = provider.elevation_range(TileCoords::from((7, 5, ZoomLevel::new(5))));
    let unknown = provider.elevation_range(TileCoords::from((0, 0, ZoomLevel::new(5))));

    assert_eq!(
        known,
        TileElevationRange {
            min_meters: 2000.0,
            max_meters: 2660.0,
        }
    );
    assert_eq!(unknown, fallback);
}

#[test]
fn an_empty_index_covers_nothing() {
    let tiles = Tiles::default();
    let index = TerrainCoverageIndex::default();

    assert!(index.is_empty());
    assert!(!index.sample(&tiles, 0.5, 0.5).covered);
    assert_eq!(index.tile_elevation_range(tile(0, 0, 0)), None);
}

#[test]
fn camera_clearance_samples_fine_cached_terrain_outside_rendered_tiles() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(0, 0, 0), gradient_tile(100.0));
    load(&mut tiles, tile(0, 0, 3), gradient_tile(5000.0));
    let index = TerrainCoverageIndex::build([tile(3, 3, 2)], &tiles, &source(1.0));
    assert_eq!(index.elevation_at(&tiles, 0.01, 0.01), None);
    assert!(
        index
            .elevation_cached(&tiles, 0.01, 0.01)
            .expect("cached mountain")
            >= 5000.0
    );
    assert!(
        index
            .elevation_cached(&tiles, 0.4, 0.4)
            .expect("coarse fallback")
            < 1000.0
    );
}

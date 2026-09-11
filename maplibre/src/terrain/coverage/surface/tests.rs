#![allow(clippy::expect_used, clippy::panic)]
use super::*;
#[test]
fn picking_and_label_height_follow_the_stitched_edge_and_transition() {
    let tile = WorldTileCoords {
        x: 3,
        y: 2,
        z: 3.into(),
    };
    let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([131, 232, 0, 255]));
    let dem = DemTile::from_image(&image, [256.0, 1.0, 1.0 / 256.0, 32768.0]).expect("DEM");
    let edges = EdgeHeights {
        samples: [[500.0; 4]; 128],
        last: [500.0; 4],
    };
    let mut index = TerrainCoverageIndex {
        exaggeration: 1.0,
        ..Default::default()
    };
    index.set_surface_edges(
        &[(Some(tile), tile, None)],
        Arc::new(HashMap::from([(tile, edges)])),
    );
    let mut tiles = Tiles::default();
    tiles
        .spawn_mut(tile)
        .expect("tile")
        .insert(DemTileComponent::Loaded(crate::terrain::LoadedDem::new(
            dem,
        )));
    let sample = |x: f64| {
        index
            .sample(&tiles, (3.0 + x / EXTENT) / 8.0, 2.5 / 8.0)
            .elevation
    };
    assert_eq!(
        sample(0.0),
        500.0,
        "pick the mesh boundary, not the unstitched DEM"
    );
    assert_eq!(sample(32.0), 750.0, "sample the same collar as the GPU");
    assert_eq!(sample(64.0), 1000.0, "keep interior detail");
}

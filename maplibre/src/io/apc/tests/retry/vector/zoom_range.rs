//! Sources that end at different zooms share one tile at the deepest of them.
use super::{
    super::{
        fixture::{Fixture, Kind},
        source::Response,
    },
    render::{assert_center, Frames},
};
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom},
    render::view_state::ViewState,
    style::source::Source,
    vector::{VectorLayerBucket, VectorLayerBucketComponent},
};

/// A polygon over the north-west quarter of its tile.
fn north_west_quarter() -> Vec<u8> {
    use geozero::mvt::{tile, Message, Tile};
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            features: vec![tile::Feature {
                id: Some(1),
                tags: vec![],
                r#type: Some(tile::GeomType::Polygon as i32),
                geometry: vec![9, 0, 0, 26, 4096, 0, 0, 4096, 4095, 0, 15],
            }],
            keys: vec![],
            values: vec![],
            extent: Some(4096),
        }],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn a_source_that_ends_sooner_is_cut_from_its_last_tile_into_the_deeper_tiles() {
    let mut test = Fixture::new(Kind::Vector, true).await;
    // The healthy source reaches zoom 1; the other ends at zoom 0, where its land covers the
    // north-west quarter of the world, so it covers the whole of the view's tile.
    if let Some(Source::Vector(healthy)) = test.context.style.sources.get_mut("healthy") {
        healthy.maxzoom = Some(1);
    }
    test.source.set_healthy(Response::Bytes(super::tile()));
    test.source.set(Response::Bytes(north_west_quarter()));
    // Three quarters of the way across tile (0, 0, 1), which spans 512 world pixels at zoom 1:
    // under the quarter once it is cut into the tile, and past it if the zoom 0 grid were
    // drawn as the tile's own.
    test.context.view_state = ViewState::new(
        crate::window::PhysicalSize::new(16, 16).expect("size"),
        WorldCoords::from((384.0, 384.0)),
        Zoom::new(1.0),
        cgmath::Deg(0.0),
        cgmath::Rad(0.64),
    );
    let mut frames = Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert!(
        test.source.requested("/healthy/1/0/0"),
        "the source that reaches the view's zoom is asked for the view's tile"
    );
    assert!(
        test.source.requested("/unstable/0/0/0"),
        "the source that ends at zoom 0 is asked for its tile there"
    );
    assert!(
        !test.source.requested("/unstable/1/"),
        "and never for a tile past its last zoom"
    );
    assert_eq!(
        test.source.requests_for("/unstable/0/0/0"),
        1,
        "the ancestor is fetched once however many tiles are cut from it"
    );
    let child = WorldTileCoords::from((0, 0, 1_u8.into()));
    let tile = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(child)
        .expect("the view's tile");
    assert!(tile.done && !tile.failed);
    let mut drawn: Vec<&str> = tile
        .layers
        .iter()
        .filter_map(|layer| match layer {
            VectorLayerBucket::AvailableLayer(bucket)
                if !bucket.buffer.buffer.indices.is_empty() =>
            {
                Some(bucket.style_layer_id.as_str())
            }
            _ => None,
        })
        .collect();
    drawn.sort_unstable();
    assert_eq!(drawn, ["healthy", "land"], "both sources reach the tile");
    assert_center(
        &frames.render(&mut test),
        [0, 255, 0, 255],
        "the quarter of the zoom 0 tile is drawn across the tile, over the deeper source",
    );
}

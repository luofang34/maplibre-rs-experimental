use super::super::super::source::Response;
use super::super::{deliver, render::assert_green, tile};
use super::*;
use geozero::mvt::{Message, Tile};

#[tokio::test]
async fn same_size_geometry_refresh_redraws_its_terrain_texture() {
    let (mut test, mut frames) = fixture().await;
    deliver(&mut test, Response::Bytes(tile())).await;
    let original = frames.render(&mut test);
    assert_green(&original);
    assert!(redraws(&test) > 0, "terrain really rendered the source");
    assert_eq!(frames.render(&mut test), original);
    assert_eq!(redraws(&test), 0, "unchanged drapes are cached");
    unrelated_upload(&mut test);
    assert_eq!(frames.render(&mut test), original);
    assert_eq!(
        redraws(&test),
        0,
        "unrelated geometry does not invalidate this drape"
    );
    let mut shifted = Tile::decode(tile().as_slice()).expect("MVT");
    shifted.layers[0].features[0].geometry = vec![9, 0, 0, 26, 2000, 0, 0, 8192, 1999, 0, 15];
    deliver(&mut test, Response::Bytes(shifted.encode_to_vec())).await;
    let changed = frames.render(&mut test);
    assert_ne!(changed, original, "actual terrain pixels change");
    assert!(redraws(&test) > 0, "new allocation invalidates its drape");
    assert_eq!(frames.render(&mut test), changed);
    assert_eq!(redraws(&test), 0);
}

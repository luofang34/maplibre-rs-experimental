use super::decode;

const TILE: &[u8] = include_bytes!("../../../../render-tests/src/assets/tiles/mlt/5/17/10.mlt");

#[test]
fn a_maplibre_tile_decodes_to_named_layers_with_features() {
    let tile = decode(TILE).expect("the fixture tile decodes");
    assert!(!tile.layers.is_empty());
    assert!(tile.layers.iter().any(|layer| !layer.features.is_empty()));
    assert!(tile.layers.iter().all(|layer| !layer.name.is_empty()));
}

#[test]
fn a_payload_that_is_not_a_tile_is_an_error() {
    assert!(decode(&[0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
}

#[test]
fn the_generic_decoder_falls_back_to_maplibre_tiles() {
    let tile = super::super::process_vector::decode_tile(TILE).expect("the tile decodes");
    assert!(!tile.layers.is_empty());
}

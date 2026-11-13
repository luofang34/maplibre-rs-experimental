#![allow(clippy::expect_used, clippy::panic)]

use super::{apply_tile_json, apply_tile_json_to_dem, TileJson};
use crate::style::source::{DemEncoding, Source, TileAddressingScheme, VectorSource};

fn source(tiles: Option<Vec<String>>, maxzoom: Option<u8>) -> VectorSource {
    VectorSource {
        maxzoom,
        tiles,
        url: Some("https://tiles.example/tiles.json".to_string()),
        ..Default::default()
    }
}

#[test]
fn parses_the_addressing_subset() {
    let document = serde_json::json!({
        "tilejson": "3.0.0",
        "name": "demo",
        "tiles": ["https://tiles.example/{z}/{x}/{y}.pbf"],
        "minzoom": 0,
        "maxzoom": 6,
        "bounds": [-180.0, -85.0, 180.0, 85.0],
        "vector_layers": [{"id": "countries"}],
        "attribution": "© Example"
    });

    let parsed: TileJson = serde_json::from_value(document).expect("parses");

    assert_eq!(
        parsed,
        TileJson {
            tiles: vec!["https://tiles.example/{z}/{x}/{y}.pbf".to_string()],
            minzoom: Some(0),
            maxzoom: Some(6),
            bounds: Some((-180.0, -85.0, 180.0, 85.0)),
            attribution: Some("© Example".to_string()),
            scheme: None,
            tile_size: None,
            encoding: None,
        }
    );
}

#[test]
fn document_fills_only_unspecified_fields() {
    let mut vector = source(None, Some(4));
    apply_tile_json(
        &mut vector,
        TileJson {
            tiles: vec!["https://tiles.example/{z}/{x}/{y}.pbf".to_string()],
            minzoom: Some(1),
            maxzoom: Some(9),
            bounds: None,
            attribution: Some("© OpenStreetMap contributors".to_string()),
            scheme: None,
            tile_size: None,
            encoding: None,
        },
    );

    assert_eq!(
        vector.tiles.as_deref(),
        Some(&["https://tiles.example/{z}/{x}/{y}.pbf".to_string()][..])
    );
    assert_eq!(vector.minzoom, Some(1));
    assert_eq!(vector.maxzoom, Some(4));
    // The provider's credit comes with its tiles, as GL JS copies it into the source.
    assert_eq!(
        vector.attribution.as_deref(),
        Some("© OpenStreetMap contributors")
    );
}

#[test]
fn explicit_tiles_are_never_replaced() {
    let mut vector = source(Some(vec!["https://mine/{z}/{x}/{y}.pbf".to_string()]), None);
    apply_tile_json(
        &mut vector,
        TileJson {
            tiles: vec!["https://theirs/{z}/{x}/{y}.pbf".to_string()],
            minzoom: None,
            maxzoom: None,
            bounds: None,
            attribution: None,
            scheme: None,
            tile_size: None,
            encoding: None,
        },
    );

    assert_eq!(
        vector.tiles.as_deref(),
        Some(&["https://mine/{z}/{x}/{y}.pbf".to_string()][..])
    );
}

/// GL JS's `loadTileJson` takes `scheme`, `tileSize` and `encoding` from the document too, so a
/// terrarium DEM such as Mapterhorn's decodes without the style repeating its encoding.
#[test]
fn dem_takes_encoding_and_tile_size_from_the_document() {
    let document = serde_json::json!({
        "tilejson": "3.0.0",
        "scheme": "xyz",
        "tiles": ["https://tiles.example/{z}/{x}/{y}.webp"],
        "encoding": "terrarium",
        "tileSize": 512
    });
    let tile_json: TileJson = serde_json::from_value(document).expect("parses");
    let Source::RasterDem(mut dem) = serde_json::from_value(serde_json::json!({
        "type": "raster-dem",
        "url": "https://tiles.example/tilejson.json"
    }))
    .expect("source parses") else {
        panic!("expected raster-dem");
    };

    apply_tile_json_to_dem(&mut dem, tile_json);

    assert_eq!(dem.encoding(), DemEncoding::Terrarium);
    assert_eq!(dem.tile_size(), 512);
}

#[test]
fn explicit_dem_encoding_and_tile_size_win_over_the_document() {
    let tile_json: TileJson = serde_json::from_value(serde_json::json!({
        "tiles": ["https://tiles.example/{z}/{x}/{y}.png"],
        "encoding": "terrarium",
        "tileSize": 512
    }))
    .expect("parses");
    let Source::RasterDem(mut dem) = serde_json::from_value(serde_json::json!({
        "type": "raster-dem",
        "url": "https://tiles.example/tilejson.json",
        "encoding": "mapbox",
        "tileSize": 256
    }))
    .expect("source parses") else {
        panic!("expected raster-dem");
    };

    apply_tile_json_to_dem(&mut dem, tile_json);

    assert_eq!(dem.encoding(), DemEncoding::Mapbox);
    assert_eq!(dem.tile_size(), 256);
}

#[test]
fn raster_takes_scheme_and_tile_size_from_the_document() {
    let mut raster = source(None, None);
    let tile_json: TileJson = serde_json::from_value(serde_json::json!({
        "tiles": ["https://tiles.example/{z}/{x}/{y}.png"],
        "scheme": "tms",
        "tileSize": 256
    }))
    .expect("parses");

    apply_tile_json(&mut raster, tile_json);

    assert_eq!(raster.scheme, Some(TileAddressingScheme::TMS));
    assert_eq!(raster.tile_size, Some(256));
}

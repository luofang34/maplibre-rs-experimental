#![allow(clippy::expect_used, clippy::panic)]

use super::dem_source;
use crate::{coords::WorldTileCoords, style::Style};

fn style(json: serde_json::Value) -> Style {
    serde_json::from_value(json).expect("style parses")
}

#[test]
fn resolves_the_terrain_source() {
    let style = style(serde_json::json!({
        "version": 8,
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"],
                    "maxzoom": 12, "tileSize": 256, "encoding": "terrarium"}
        },
        "layers": [],
        "terrain": {"source": "dem", "exaggeration": 1.5}
    }));

    let dem = dem_source(&style).expect("terrain source resolves");
    assert_eq!(dem.name, "dem");
    assert_eq!(dem.tile_size, 256);
    assert_eq!(dem.minzoom, 0);
    assert_eq!(dem.maxzoom, 12);
    assert_eq!(dem.exaggeration, 1.5);
    assert_eq!(dem.unpack, [256.0, 1.0, 1.0 / 256.0, 32768.0]);
    assert_eq!(
        dem.source.format(&WorldTileCoords::from((3, 5, 4.into()))),
        Some("https://dem.example/4/3/5.png".to_string())
    );
}

#[test]
fn missing_terrain_or_tiles_yields_none() {
    let no_terrain = style(serde_json::json!({"version": 8, "sources": {}, "layers": []}));
    assert!(dem_source(&no_terrain).is_none());

    let unresolved = style(serde_json::json!({
        "version": 8,
        "sources": {"dem": {"type": "raster-dem", "url": "https://dem.example/tiles.json"}},
        "layers": [],
        "terrain": {"source": "dem"}
    }));
    assert!(dem_source(&unresolved).is_none());

    let wrong_kind = style(serde_json::json!({
        "version": 8,
        "sources": {"dem": {"type": "raster", "tiles": ["https://r.example/{z}/{x}/{y}.png"]}},
        "layers": [],
        "terrain": {"source": "dem"}
    }));
    assert!(dem_source(&wrong_kind).is_none());
}

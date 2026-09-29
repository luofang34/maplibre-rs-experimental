#![allow(clippy::expect_used)]

use super::*;

#[test]
fn the_roll_of_the_view_is_read_from_the_style() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "sources": {}, "layers": [], "bearing": 30, "pitch": 40, "roll": 15
    }))
    .expect("style parses");
    assert_eq!(style.roll, Some(15.0));
    let plain: Style =
        serde_json::from_value(serde_json::json!({"version": 8, "sources": {}, "layers": []}))
            .expect("style parses");
    assert_eq!(plain.roll, None);
}

#[test]
fn test_reading() {
    // language=JSON
    let style_json_str = r##"
    {
      "version": 8,
      "name": "Test Style",
      "metadata": {},
      "sources": {
        "openmaptiles": {
          "type": "vector",
          "url": "https://maps.tuerantuer.org/europe_germany/tiles.json"
        }
      },
      "layers": [
        {
          "id": "background",
          "type": "background",
          "paint": {"background-color": "rgb(239,239,239)"}
        },
        {
          "id": "transportation",
          "type": "line",
          "source": "openmaptiles",
          "source-layer": "transportation",
          "paint": {
            "line-color": "#3D3D3D"
          }
        },
        {
          "id": "boundary",
          "type": "line",
          "source": "openmaptiles",
          "source-layer": "boundary",
          "paint": {
            "line-color": "#3D3D3D"
          }
        },
        {
          "id": "building",
          "minzoom": 14,
          "maxzoom": 15,
          "type": "fill",
          "source": "openmaptiles",
          "source-layer": "building",
          "paint": {
            "line-color": "#3D3D3D"
          }
        }
      ]
    }
    "##;

    let _style: Style = serde_json::from_str(style_json_str).expect("valid test style value");
}

#[test]
fn test_style_roundtrip_serde() {
    let style = Style::default();
    let json = serde_json::to_string(&style).expect("valid test style value");
    let roundtripped: Style = serde_json::from_str(&json).expect("valid test style value");
    assert_eq!(
        serde_json::to_value(&style).expect("original document"),
        serde_json::to_value(&roundtripped).expect("worker document"),
        "worker serialization must preserve paint, sources and view settings"
    );
}

#[test]
fn parses_the_gl_js_3d_terrain_example() {
    // language=JSON
    let style_json_str = r##"
    {
      "version": 8,
      "sources": {
        "osm": {"type": "raster", "tiles": ["https://a.tile.openstreetmap.org/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 19},
        "terrainSource": {"type": "raster-dem", "url": "https://demotiles.maplibre.org/terrain-tiles/tiles.json", "tileSize": 256},
        "hillshadeSource": {"type": "raster-dem", "url": "https://demotiles.maplibre.org/terrain-tiles/tiles.json", "tileSize": 256}
      },
      "layers": [
        {"id": "osm", "type": "raster", "source": "osm"},
        {"id": "hills", "type": "hillshade", "source": "hillshadeSource", "paint": {"hillshade-shadow-color": "#473B24"}}
      ],
      "terrain": {"source": "terrainSource", "exaggeration": 1},
      "sky": {}
    }
    "##;
    let style: Style = serde_json::from_str(style_json_str).expect("valid test style value");

    let terrain = style.terrain.expect("terrain root property");
    assert_eq!(terrain.source, "terrainSource");
    assert_eq!(terrain.exaggeration, 1.0);
    match style.sources.get("terrainSource") {
        Some(Source::RasterDem(source)) => assert_eq!(source.tile_size, 256),
        other => panic!("expected raster-dem source, got {other:?}"),
    }
}

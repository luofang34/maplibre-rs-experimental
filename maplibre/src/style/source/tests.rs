#![allow(clippy::expect_used, clippy::panic)]

use super::{DemEncoding, RasterDemSource, Source};

fn parse(json: &str) -> RasterDemSource {
    match serde_json::from_str::<Source>(json).expect("source parses") {
        Source::RasterDem(source) => source,
        other => panic!("expected raster-dem, got {other:?}"),
    }
}

#[test]
fn raster_dem_defaults_follow_the_style_spec() {
    let source = parse(r#"{"type": "raster-dem", "url": "https://tiles.example/tiles.json"}"#);

    assert_eq!(source.tile_size, 512);
    assert_eq!(source.encoding, DemEncoding::Mapbox);
    assert_eq!(source.unpack_vector(), [6553.6, 25.6, 0.1, 10000.0]);
    assert_eq!(
        source.url.as_deref(),
        Some("https://tiles.example/tiles.json")
    );
}

#[test]
fn terrarium_and_custom_unpack_vectors() {
    let terrarium = parse(
        r#"{"type": "raster-dem", "tiles": ["t/{z}/{x}/{y}.png"], "encoding": "terrarium", "tileSize": 256}"#,
    );
    assert_eq!(terrarium.tile_size, 256);
    assert_eq!(
        terrarium.unpack_vector(),
        [256.0, 1.0, 1.0 / 256.0, 32768.0]
    );

    let custom = parse(
        r#"{"type": "raster-dem", "tiles": ["t/{z}/{x}/{y}.png"], "encoding": "custom",
            "redFactor": 2.0, "greenFactor": 3.0, "blueFactor": 4.0, "baseShift": 5.0}"#,
    );
    assert_eq!(custom.unpack_vector(), [2.0, 3.0, 4.0, 5.0]);
}

#[test]
fn unpack_vectors_recover_known_elevations() {
    let mapbox = parse(r#"{"type": "raster-dem", "tiles": ["t/{z}/{x}/{y}.png"]}"#);
    let [r, g, b, base] = mapbox.unpack_vector();
    // 0x01, 0x86, 0xA0 encodes 10000 * 0.1 + (1 * 65536 + 134 * 256 + 160) / 10 - 10000
    let elevation = 1.0 * r + 134.0 * g + 160.0 * b - base;
    assert!((elevation - 0.0).abs() < 1e-9, "sea level, got {elevation}");

    let terrarium =
        parse(r#"{"type": "raster-dem", "tiles": ["t/{z}/{x}/{y}.png"], "encoding": "terrarium"}"#);
    let [r, g, b, base] = terrarium.unpack_vector();
    let elevation = 128.0 * r + 0.0 * g + 0.0 * b - base;
    assert!((elevation - 0.0).abs() < 1e-9, "sea level, got {elevation}");
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use flate2::{
    write::{GzEncoder, ZlibEncoder},
    Compression,
};
use rusqlite::{params, Connection};

use super::{MbtilesClient, MbtilesUrl};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::{
        resource_loader::SharedLoader,
        source_client::{
            HttpClient, HttpSourceClient, NetworkState, SourceClient, SourceFetchError,
        },
        source_type::{SourceType, TessellateSource},
        tile_json::{resolve_tile_json_sources, TileJson},
    },
    style::{
        source::{Source, TileAddressingScheme, VectorSource},
        Style,
    },
};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(1);

struct TempMbtiles {
    path: PathBuf,
}

impl TempMbtiles {
    fn create(setup: impl FnOnce(&Connection)) -> Self {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "maplibre-runtime-mbtiles-{}-{id}.mbtiles",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).expect("create temp mbtiles");
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
             CREATE UNIQUE INDEX tile_index on tiles (zoom_level, tile_column, tile_row);",
        )
        .expect("create mbtiles schema");
        setup(&conn);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn archive_url(&self) -> String {
        format!("mbtiles://{}", self.path.display())
    }
}

impl Drop for TempMbtiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Default)]
struct RejectAllHttp;

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for RejectAllHttp {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        Err(SourceFetchError(Box::new(std::io::Error::other(format!(
            "unexpected inner HTTP call for {url}"
        )))))
    }
}

fn gzip_bytes(payload: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(payload).expect("gzip write");
    enc.finish().expect("gzip finish")
}

fn zlib_bytes(payload: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(payload).expect("zlib write");
    enc.finish().expect("zlib finish")
}

#[test]
fn url_parser_handles_relative_absolute_and_tile_coordinates() {
    assert_eq!(
        MbtilesUrl::parse("mbtiles://data/world.mbtiles"),
        Some(MbtilesUrl::Archive("data/world.mbtiles".to_owned()))
    );
    assert_eq!(
        MbtilesUrl::parse("mbtiles:///var/maps/world.mbtiles"),
        Some(MbtilesUrl::Archive("/var/maps/world.mbtiles".to_owned()))
    );
    assert_eq!(
        MbtilesUrl::parse("mbtiles:///var/maps/world.mbtiles/4/3/5"),
        Some(MbtilesUrl::Tile {
            archive: "/var/maps/world.mbtiles".to_owned(),
            z: 4,
            x: 3,
            y: 5,
        })
    );
    assert_eq!(
        MbtilesUrl::parse("mbtiles://rel/world.mbtiles/2/1/3.pbf"),
        Some(MbtilesUrl::Tile {
            archive: "rel/world.mbtiles".to_owned(),
            z: 2,
            x: 1,
            y: 3,
        })
    );
    assert_eq!(MbtilesUrl::parse("https://example.com/0/0/0.pbf"), None);
}

#[tokio::test]
async fn runtime_mbtiles_serves_tilejson_decompresses_mvt_passes_png_and_flips_tms_y() {
    let mvt_payload = b"\x1a\x08mock-mvt-layer-bytes";
    let zlib_payload = b"\x1a\x09zlib-mvt-layer-bytes";
    let png_payload = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDRmock-png";

    let archive = TempMbtiles::create(|conn| {
        for (k, v) in [
            ("name", "Test Vector & Raster"),
            ("format", "pbf"),
            ("minzoom", "0"),
            ("maxzoom", "6"),
            ("bounds", "-10.5,40.0,15.25,55.5"),
            ("center", "2.35,48.85,4"),
            ("attribution", "MapLibre Test"),
            ("scheme", "tms"),
            ("json", r#"{"vector_layers":[{"id":"water","fields":{}}]}"#),
        ] {
            conn.execute(
                "INSERT INTO metadata (name, value) VALUES (?1, ?2)",
                params![k, v],
            )
            .expect("insert metadata");
        }

        // XYZ (z=2, x=1, y=0) -> TMS row = (1 << 2) - 1 - 0 = 3 (gzipped MVT)
        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (2, 1, 3, ?1)",
            params![gzip_bytes(mvt_payload)],
        )
        .expect("insert gzip tile");

        // XYZ (z=2, x=2, y=1) -> TMS row = (1 << 2) - 1 - 1 = 2 (zlib MVT)
        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (2, 2, 2, ?1)",
            params![zlib_bytes(zlib_payload)],
        )
        .expect("insert zlib tile");

        // XYZ (z=3, x=4, y=2) -> TMS row = (1 << 3) - 1 - 2 = 5 (raw PNG)
        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (3, 4, 5, ?1)",
            params![png_payload.as_slice()],
        )
        .expect("insert png tile");
    });

    let client = MbtilesClient::new(RejectAllHttp);
    let archive_url = archive.archive_url();

    // 1. Fetch synthesized TileJSON and verify fields + XYZ scheme override.
    let raw_json = client.fetch(&archive_url).await.expect("TileJSON fetch");
    let parsed: TileJson = serde_json::from_slice(&raw_json).expect("valid TileJson");
    assert_eq!(parsed.minzoom, Some(0));
    assert_eq!(parsed.maxzoom, Some(6));
    assert_eq!(parsed.bounds, Some((-10.5, 40.0, 15.25, 55.5)));
    assert_eq!(parsed.attribution.as_deref(), Some("MapLibre Test"));
    assert_eq!(parsed.scheme, Some(TileAddressingScheme::XYZ));
    assert_eq!(
        parsed.tiles,
        vec![format!("{archive_url}/{{z}}/{{x}}/{{y}}")]
    );

    let full_doc: serde_json::Value = serde_json::from_slice(&raw_json).expect("json value");
    assert_eq!(full_doc["vector_layers"][0]["id"], "water");

    // 2. Fetch gzipped MVT tile via XYZ (2, 1, 0) -> decompressed cleanly.
    let got_mvt = client
        .fetch(&format!("{archive_url}/2/1/0"))
        .await
        .expect("gzip MVT tile");
    assert_eq!(got_mvt, mvt_payload);

    // 3. Fetch zlib MVT tile via XYZ (2, 2, 1) -> decompressed cleanly.
    let got_zlib = client
        .fetch(&format!("{archive_url}/2/2/1"))
        .await
        .expect("zlib MVT tile");
    assert_eq!(got_zlib, zlib_payload);

    // 4. Fetch PNG raster tile via XYZ (3, 4, 2) -> untouched raw PNG bytes.
    let got_png = client
        .fetch(&format!("{archive_url}/3/4/2"))
        .await
        .expect("PNG tile");
    assert_eq!(got_png, png_payload);

    // 5. Missing tile row (including unflipped TMS coordinate) returns `is_not_found() == true`.
    let missing = client
        .fetch(&format!("{archive_url}/2/1/3"))
        .await
        .expect_err("unflipped coordinate must be absent");
    assert!(missing.is_not_found());
    assert!(!missing.is_retryable());
}

#[tokio::test]
async fn mbtiles_resolves_through_shared_loader_and_works_while_offline() {
    let expected = b"\x1a\x06offline-mbtiles-tile";
    let archive = TempMbtiles::create(|conn| {
        conn.execute(
            "INSERT INTO metadata (name, value) VALUES ('minzoom', '0'), ('maxzoom', '4')",
            [],
        )
        .expect("metadata");
        // XYZ (1, 0, 1) -> TMS row = (1 << 1) - 1 - 1 = 0
        conn.execute(
            "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (1, 0, 0, ?1)",
            params![gzip_bytes(expected)],
        )
        .expect("tile");
    });

    let loader = SharedLoader::new(RejectAllHttp);
    let source_client = SourceClient::new(
        HttpSourceClient::new(loader).with_network_state(NetworkState::offline()),
    );
    assert!(!source_client.is_online());

    let mut sources = HashMap::new();
    sources.insert(
        "local_mb".to_owned(),
        Source::Vector(VectorSource {
            url: Some(archive.archive_url()),
            ..VectorSource::default()
        }),
    );
    let mut style = Style {
        sources,
        ..Style::default()
    };

    resolve_tile_json_sources(&mut style, &source_client).await;

    let Source::Vector(resolved) = &style.sources["local_mb"] else {
        panic!("expected vector source");
    };
    let templates = resolved
        .tiles
        .clone()
        .expect("tiles populated from mbtiles");
    let source_type = SourceType::Tessellate(TessellateSource::from_template(
        templates[0].clone(),
        resolved.scheme.unwrap_or(TileAddressingScheme::XYZ),
    ));

    let coords = WorldTileCoords::from((0, 1, ZoomLevel::new(1)));
    let tile = source_client
        .fetch(&coords, &source_type)
        .await
        .expect("mbtiles tile loads through SharedLoader even when Offline");
    assert_eq!(tile, expected);
    assert!(archive.path().exists());
}

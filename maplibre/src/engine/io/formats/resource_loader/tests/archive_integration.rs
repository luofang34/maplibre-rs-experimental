use crate::io::{
    pmtiles::tests::{archive, tiles, ArchiveServer, ARCHIVE},
    resource_loader::SharedLoader,
    source_client::{
        ByteRange, HttpClient, HttpRequest, HttpSourceClient, NetworkState, NetworkStatus,
        ResourceKind, SourceClient, SourceFetchError,
    },
};

#[tokio::test]
async fn data_urls_and_local_files_succeed_while_network_state_is_offline() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use async_trait::async_trait;

    #[derive(Clone, Default)]
    struct FailHttp;

    #[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
    #[cfg_attr(feature = "thread-safe-futures", async_trait)]
    impl HttpClient for FailHttp {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
            Err(SourceFetchError(Box::new(std::io::Error::other(format!(
                "unexpected network call for {url}"
            )))))
        }
    }

    let transitions = Arc::new(AtomicUsize::new(0));
    let transitions_clone = Arc::clone(&transitions);
    let net = NetworkState::new();
    net.on_change(move |_status: NetworkStatus| {
        transitions_clone.fetch_add(1, Ordering::SeqCst);
    });
    assert!(net.is_online());
    assert_eq!(net.generation(), 0);

    let client = HttpSourceClient::new(SharedLoader::new(FailHttp)).with_network_state(net.clone());
    client.set_online(false);
    assert!(!client.is_online());
    assert_eq!(net.generation(), 1);
    assert_eq!(transitions.load(Ordering::SeqCst), 1);

    let plain_data = client
        .fetch_url("data:application/geo+json,%7B%22type%22%3A%22FeatureCollection%22%7D")
        .await
        .expect("percent-encoded data URL");
    assert_eq!(plain_data, br#"{"type":"FeatureCollection"}"#);

    let b64_data = client
        .fetch_request(
            HttpRequest::new(
                "data:application/octet-stream;base64,aGVsbG8gd29ybGQ=",
                ResourceKind::Tile,
            )
            .with_range(ByteRange {
                offset: 6,
                length: 5,
            }),
        )
        .await
        .expect("base64 data URL range");
    assert_eq!(b64_data, b"world");

    let temp_file = std::env::temp_dir().join(format!(
        "maplibre-offline-file-{}.geojson",
        std::process::id()
    ));
    std::fs::write(&temp_file, br#"{"type":"Point","coordinates":[0,0]}"#).expect("write temp");
    let file_bytes = client
        .fetch_url(&format!("file://{}", temp_file.display()))
        .await
        .expect("file:// URL while offline");
    drop(std::fs::remove_file(&temp_file));
    assert_eq!(file_bytes, br#"{"type":"Point","coordinates":[0,0]}"#);

    let remote_err = client
        .fetch_url("https://tiles.example.test/1/0/0.pbf")
        .await
        .expect_err("remote HTTP must fail while offline");
    assert!(remote_err.is_offline());
    assert!(remote_err.is_retryable());
}

#[tokio::test]
async fn a_shared_loader_reads_archives_too() {
    let server = ArchiveServer::new(archive(&tiles(), true));
    let loader = SharedLoader::new(server.clone());
    assert_eq!(
        loader
            .fetch(&format!("pmtiles://{ARCHIVE}/1/1/0"))
            .await
            .expect("tile"),
        b"one-east"
    );
}

#[cfg(feature = "headless")]
fn road_tile() -> Vec<u8> {
    use geozero::mvt::Message as _;
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(geozero::mvt::tile::GeomType::Linestring as i32),
                geometry: vec![9, 2, 2, 10, 2, 2],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

#[cfg(feature = "headless")]
#[tokio::test]
async fn a_map_draws_vector_tiles_out_of_an_archive() {
    use crate::{
        headless::{create_headless_renderer_with_loader, map::HeadlessMap},
        io::tile_retry::RequestKind,
        map::RenderPlugin,
        render::frame_signals::ResourceReady,
        style::Style,
        vector::{DefaultVectorTransferables, VectorPlugin},
    };
    let tiles: Vec<_> = (0..4)
        .flat_map(|x| (0..4).map(move |y| ((2, x, y), road_tile())))
        .chain([((0, 0, 0), road_tile())])
        .collect();
    let server = ArchiveServer::new(archive(&tiles, true));
    let (kernel, renderer) = create_headless_renderer_with_loader(
        64,
        64,
        Default::default(),
        SharedLoader::new(server.clone()),
    )
    .await
    .expect("renderer");
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": 2,
        "sources": {"v": {"type": "vector",
            "tiles": [format!("pmtiles://{ARCHIVE}/{{z}}/{{x}}/{{y}}")]}},
        "layers": [{"id": "roads", "type": "line", "source": "v", "source-layer": "roads"}]
    }))
    .expect("style");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        ],
    )
    .expect("map");
    let mut loaded = 0;
    for _ in 0..50 {
        map.run_frame().expect("frame");
        loaded += map
            .take_ready_resources()
            .iter()
            .filter(|resource| {
                matches!(
                    resource,
                    ResourceReady::Tile {
                        kind: RequestKind::Vector,
                        loaded: true,
                        ..
                    }
                )
            })
            .count();
        if loaded >= 4 && !map.needs_redraw() {
            break;
        }
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            server.served.notified(),
        )
        .await
        .ok();
    }
    assert!(
        loaded >= 4,
        "vector tiles load out of the archive: {loaded}"
    );
    let ranges = server.ranges.lock().expect("ranges").len();
    assert!(
        ranges < loaded + 4,
        "the header and directories are read once, then a range per tile: {ranges} ranges for {loaded} tiles"
    );
}

#[tokio::test]
async fn a_source_naming_an_archive_resolves_to_its_tiles() {
    use crate::style::{source::Source, Style};
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {"v": {"type": "vector", "url": format!("pmtiles://{ARCHIVE}")}},
        "layers": []
    }))
    .expect("style");
    let client = SourceClient::new(HttpSourceClient::new(SharedLoader::new(
        ArchiveServer::new(archive(&tiles(), false)),
    )));
    crate::io::tile_json::resolve_tile_json_sources(&mut style, &client).await;
    assert!(matches!(style.sources.get("v"), Some(Source::Vector(_))));
    if let Some(Source::Vector(source)) = style.sources.get("v") {
        assert_eq!(
            source.tiles.as_deref(),
            Some(&[format!("pmtiles://{ARCHIVE}/{{z}}/{{x}}/{{y}}")][..])
        );
    }
}

#[cfg(feature = "native-io")]
#[tokio::test]
async fn mbtiles_resolves_through_shared_loader_and_works_while_offline() {
    use rusqlite::params;
    use rustc_hash::FxHashMap;

    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        io::{
            mbtiles::tests::{gzip_bytes, RejectAllHttp, TempMbtiles},
            source_type::{SourceType, TessellateSource},
            tile_json::resolve_tile_json_sources,
        },
        style::{
            source::{Source, TileAddressingScheme, VectorSource},
            Style,
        },
    };

    let expected = b"\x1a\x06offline-mbtiles-tile";
    let archive = TempMbtiles::create(|conn| {
        conn.execute(
            "INSERT INTO metadata (name, value) VALUES ('minzoom', '0'), ('maxzoom', '4')",
            [],
        )
        .expect("metadata");
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

    let mut sources = FxHashMap::default();
    sources.insert(
        "local_mb".to_owned(),
        Source::Vector(VectorSource {
            url: Some(archive.archive_url()),
            ..VectorSource::default()
        }),
    );
    let mut style = Style::from_document(crate::style::StyleDocument {
        sources,
        ..Default::default()
    });

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

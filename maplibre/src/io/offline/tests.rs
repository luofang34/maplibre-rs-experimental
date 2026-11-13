#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
#[cfg(not(target_arch = "wasm32"))]
use crate::io::source_client::ResourceKind;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    style::Style,
};

#[test]
fn tile_pyramid_handles_full_world_zoom_clamping_and_antimeridian_crossing() {
    let full = OfflineRegionDefinition::from_style_json("{}", [-180.0, -85.05, 180.0, 85.05], 0, 2);
    let coords = full.tile_pyramid();
    // z=0: 1, z=1: 4, z=2: 16 -> 21 total tiles in deterministic (z, x, y) order.
    assert_eq!(coords.len(), 21);
    assert_eq!(coords[0], WorldTileCoords::from((0, 0, ZoomLevel::new(0))));

    // Source zoom clamping [1..=1] yields only the 4 z=1 tiles.
    let clamped = full.tile_pyramid_for_source(Some(1), Some(1), None);
    assert_eq!(clamped.len(), 4);

    // Antimeridian crossing: [170, -10, -170, 10] at z=2 (grid 4x4):
    // x=3 (170..180) and x=0 (-180..-170), y=1..=2 (-10..10 crosses equator).
    let dateline =
        OfflineRegionDefinition::from_style_json("{}", [170.0, -10.0, -170.0, 10.0], 2, 2);
    let dl_tiles = dateline.tile_pyramid();
    let xs: Vec<i32> = dl_tiles.iter().map(|c| c.x).collect();
    assert!(xs.contains(&0) && xs.contains(&3));
    assert!(!xs.contains(&1) && !xs.contains(&2));
}

#[test]
fn resource_enumeration_skips_volatile_sources_and_includes_sprites_and_glyphs() {
    let style_json = serde_json::json!({
        "version": 8,
        "sprite": "https://cdn.example.test/sprites/basic",
        "glyphs": "https://cdn.example.test/fonts/{fontstack}/{range}.pbf",
        "sources": {
            "roads": { "type": "vector", "tiles": ["https://cdn.example.test/v/{z}/{x}/{y}.pbf"], "minzoom": 0, "maxzoom": 1 },
            "live_traffic": { "type": "vector", "tiles": ["https://cdn.example.test/traffic/{z}/{x}/{y}.pbf"], "volatile": true }
        },
        "layers": []
    })
    .to_string();
    let style: Style = serde_json::from_str(&style_json).expect("valid style");
    let def = OfflineRegionDefinition::from_style_json(style_json, [2.0, 48.0, 2.5, 49.0], 0, 1);
    let resources = enumerate_style_resources(&style, &def);

    let urls: Vec<&str> = resources.iter().map(|r| r.url.as_str()).collect();
    assert!(urls.contains(&"https://cdn.example.test/sprites/basic.json"));
    assert!(urls.contains(&"https://cdn.example.test/sprites/basic.png"));
    assert!(urls.contains(&"https://cdn.example.test/fonts/Open%20Sans%20Regular/0-255.pbf"));
    assert!(urls
        .iter()
        .any(|u| u.starts_with("https://cdn.example.test/v/")));
    assert!(
        !urls.iter().any(|u| u.contains("/traffic/")),
        "volatile sources must never be enumerated into offline packs"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn memory_offline_store_deduplicates_evicts_and_invalidates() {
    futures::executor::block_on(async {
        let store = MemoryOfflineStore::new();
        let def = OfflineRegionDefinition::from_style_json("{}", [0.0, 0.0, 1.0, 1.0], 0, 0);
        let r1 = store.create_region(&def).await.expect("r1");
        let r2 = store.create_region(&def).await.expect("r2");
        store
            .put_region_resource(r1, "https://t.test/0/0/0.pbf", ResourceKind::Tile, b"abc")
            .await
            .expect("put r1");
        store
            .link_existing_resource(r2, "https://t.test/0/0/0.pbf")
            .await
            .expect("link r2");
        store.delete_region(r1).await.expect("delete r1");
        assert_eq!(
            store
                .get_resource("https://t.test/0/0/0.pbf")
                .await
                .expect("get")
                .as_deref(),
            Some(b"abc".as_slice())
        );
        store.invalidate_region(r2).await.expect("invalidate r2");
        assert!(!store.region_status(r2).await.expect("status").complete);
    });
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
mod native_sqlite_tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use futures::channel::mpsc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;
    use crate::{
        io::{
            source_client::{HttpSourceClient, NetworkState, SourceClient},
            source_type::{SourceType, TessellateSource},
            tile_json::resolve_tile_json_sources,
        },
        platform::http_client::{InProcessRoutes, ReqwestHttpClient},
        style::source::{Source, TileAddressingScheme},
    };

    static TEMP_DB_SEQ: AtomicUsize = AtomicUsize::new(1);

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        let seq = TEMP_DB_SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "maplibre-offline-{tag}-{}-{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn build_in_process_fixture(
        prefix: &str,
        request_counter: Arc<AtomicUsize>,
    ) -> (InProcessRoutes, String) {
        let routes = InProcessRoutes::new();
        let base = prefix.to_owned();
        let style_url = format!("{base}/style.json");
        let tilejson_url = format!("{base}/roads.json");
        let style_doc = serde_json::json!({
            "version": 8,
            "sprite": format!("{base}/sprite"),
            "glyphs": format!("{base}/fonts/{{fontstack}}/{{range}}.pbf"),
            "sources": { "roads": { "type": "vector", "url": tilejson_url } },
            "layers": []
        })
        .to_string();

        let tilejson_doc = serde_json::json!({
            "tilejson": "3.0.0",
            "tiles": [format!("{base}/tiles/{{z}}/{{x}}/{{y}}.pbf")],
            "minzoom": 0,
            "maxzoom": 2
        })
        .to_string();

        routes.register(
            prefix,
            Arc::new(move |url, _range| {
                request_counter.fetch_add(1, Ordering::SeqCst);
                let u = url.to_owned();
                let s_doc = style_doc.clone();
                let tj_doc = tilejson_doc.clone();
                Box::pin(async move {
                    if u.ends_with("/style.json") {
                        Ok(s_doc.into_bytes())
                    } else if u.ends_with("/roads.json") {
                        Ok(tj_doc.into_bytes())
                    } else if u.ends_with("/sprite.json") {
                        Ok(b"{}".to_vec())
                    } else if u.ends_with("/sprite.png") {
                        Ok(b"\x89PNG\r\n\x1a\nmock-sprite-png".to_vec())
                    } else if u.ends_with("/0-255.pbf") {
                        Ok(vec![0x0a; 256])
                    } else if u.contains("/tiles/") {
                        // Compressible >128B vector tile payload so SQLite zlib path is exercised.
                        Ok(format!("tile-payload:{u}:{}", "x".repeat(200)).into_bytes())
                    } else {
                        Err(crate::io::source_client::SourceFetchError::not_found(&u))
                    }
                })
            }),
        );

        (routes, style_url)
    }

    #[tokio::test]
    async fn end_to_end_download_serves_entire_region_offline_after_server_shutdown() {
        // Test against a real loopback TCP HTTP server when permitted, falling back to
        // `InProcessRoutes` only in restricted sandboxes without loopback socket bind.
        let hits = Arc::new(AtomicUsize::new(0));
        let (raw_http, style_url, server_handle, routes_opt) =
            if let Ok(listener) = TcpListener::bind("127.0.0.1:0").await {
                let addr = listener.local_addr().expect("addr");
                let base = format!("http://{addr}");
                let style_url = format!("{base}/style.json");
                let tj_url = format!("{base}/roads.json");
                let style_doc = serde_json::json!({
                    "version": 8,
                    "sprite": format!("{base}/sprite"),
                    "glyphs": format!("{base}/fonts/{{fontstack}}/{{range}}.pbf"),
                    "sources": { "roads": { "type": "vector", "url": tj_url } },
                    "layers": []
                })
                .to_string();
                let tj_doc = serde_json::json!({
                    "tilejson": "3.0.0",
                    "tiles": [format!("{base}/tiles/{{z}}/{{x}}/{{y}}.pbf")],
                    "minzoom": 0,
                    "maxzoom": 2
                })
                .to_string();
                let srv_hits = Arc::clone(&hits);
                let task = tokio::spawn(async move {
                    while let Ok((mut stream, _)) = listener.accept().await {
                        srv_hits.fetch_add(1, Ordering::SeqCst);
                        let mut buf = vec![0_u8; 2048];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let path = req
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let body: Vec<u8> = if path == "/style.json" {
                            style_doc.as_bytes().to_vec()
                        } else if path == "/roads.json" {
                            tj_doc.as_bytes().to_vec()
                        } else if path == "/sprite.json" {
                            b"{}".to_vec()
                        } else if path == "/sprite.png" {
                            b"\x89PNG\r\n\x1a\nmock-sprite".to_vec()
                        } else if path.ends_with("/0-255.pbf") {
                            vec![0x0a; 256]
                        } else {
                            format!("tile:{path}:{}", "z".repeat(200)).into_bytes()
                        };
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes()).await;
                        let _ = stream.write_all(&body).await;
                    }
                });
                (
                    ReqwestHttpClient::new::<String>(None),
                    style_url,
                    Some(task),
                    None,
                )
            } else {
                let prefix = "http://127.0.0.1:1/offline-e2e";
                let (routes, style_url) = build_in_process_fixture(prefix, Arc::clone(&hits));
                (
                    ReqwestHttpClient::with_in_process_routes::<String>(None, routes.clone()),
                    style_url,
                    None,
                    Some((routes, prefix)),
                )
            };

        let db_path = temp_db_path("e2e");
        let store = Arc::new(SqliteOfflineStore::open(&db_path).expect("open sqlite store"));
        let network = NetworkState::new();
        let offline_http = OfflineHttpClient::new(raw_http, Arc::clone(&store));
        let source_client = SourceClient::new(
            HttpSourceClient::new(offline_http).with_network_state(network.clone()),
        );

        let manager =
            OfflineManager::new(source_client.clone(), Arc::clone(&store)).with_concurrency(4);
        let definition =
            OfflineRegionDefinition::from_style_url(&style_url, [2.2, 48.8, 2.4, 48.9], 0, 2)
                .with_metadata(b"paris-pack".to_vec());

        let (tx, mut rx) = mpsc::unbounded();
        let summary = manager
            .download_region_to_channel(definition.clone(), OfflineCancellationToken::new(), tx)
            .await
            .expect("download region succeeds");
        assert!(summary.status.complete);
        assert_eq!(summary.definition.metadata, b"paris-pack");
        // Paris small bbox hits 1 tile per zoom across z=0,1,2 -> 3 tiles + style + TileJSON + 2 sprites + 1 glyph = 8 resources.
        assert_eq!(summary.status.completed_tile_count, 3);
        assert_eq!(summary.status.completed_resource_count, 8);

        let mut saw_complete_event = false;
        while let Ok(ev) = rx.try_recv() {
            if ev.state == OfflineDownloadState::Complete {
                saw_complete_event = true;
            }
        }
        assert!(saw_complete_event);

        // Shut down server completely AND switch NetworkState to Offline!
        if let Some(task) = server_handle {
            task.abort();
        }
        if let Some((routes, prefix)) = routes_opt {
            routes.unregister(prefix);
        }
        network.set_online(false);
        let hits_before_offline = hits.load(Ordering::SeqCst);

        // Verify style, TileJSON resolution, sprites, glyphs, and all 3 zoom-level tiles load 100% offline!
        let offline_style_bytes = source_client
            .fetch_url(&style_url)
            .await
            .expect("style loads offline");
        let mut offline_style: Style =
            serde_json::from_slice(&offline_style_bytes).expect("parse offline style");
        resolve_tile_json_sources(&mut offline_style, &source_client).await;

        let Source::Vector(roads) = &offline_style.sources["roads"] else {
            panic!("expected vector source");
        };
        let tpl = roads.tiles.as_ref().expect("offline TileJSON resolved")[0].clone();
        let st = SourceType::Tessellate(TessellateSource::from_template(
            tpl,
            TileAddressingScheme::XYZ,
        ));
        for coords in definition.tile_pyramid() {
            let tile_bytes = source_client
                .fetch(&coords, &st)
                .await
                .expect("every region tile must load offline");
            assert!(tile_bytes.starts_with(b"tile"));
        }

        // Unpacked tile outside Paris bbox fails immediately with `OfflineError` while offline.
        let outside = WorldTileCoords::from((3, 3, ZoomLevel::new(2)));
        let miss = source_client
            .fetch(&outside, &st)
            .await
            .expect_err("tile outside downloaded region must fail offline");
        assert!(miss.is_offline());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            hits_before_offline,
            "zero origin hits while offline"
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn deduplication_delete_invalidate_quotas_cancellation_and_sideload_merge_work() {
        let hits = Arc::new(AtomicUsize::new(0));
        let prefix = "http://127.0.0.1:1/offline-mgmt";
        let (routes, style_url) = build_in_process_fixture(prefix, Arc::clone(&hits));
        let http = ReqwestHttpClient::with_in_process_routes::<String>(None, routes);

        let main_path = temp_db_path("main");
        let side_path = temp_db_path("sideload");
        let store = Arc::new(SqliteOfflineStore::open(&main_path).expect("main db"));
        let client = SourceClient::new(HttpSourceClient::new(OfflineHttpClient::new(
            http.clone(),
            Arc::clone(&store),
        )));
        let manager = OfflineManager::new(client.clone(), Arc::clone(&store)).with_concurrency(2);

        // 1. Overlapping regions deduplicate shared tiles; deleting Region 1 preserves shared z=0 tile for Region 2!
        let reg1_def =
            OfflineRegionDefinition::from_style_url(&style_url, [2.0, 48.0, 2.5, 49.0], 0, 1);
        let reg2_def =
            OfflineRegionDefinition::from_style_url(&style_url, [2.1, 48.1, 2.6, 49.1], 0, 2);

        let r1 = manager
            .download_region(reg1_def, OfflineCancellationToken::new(), |_| {})
            .await
            .expect("r1");
        let hits_after_r1 = hits.load(Ordering::SeqCst);

        let r2 = manager
            .download_region(reg2_def, OfflineCancellationToken::new(), |_| {})
            .await
            .expect("r2");
        // Only the single new z=2 tile needed a network fetch; style, TileJSON, sprites, glyphs, z=0, z=1 were deduplicated!
        assert_eq!(hits.load(Ordering::SeqCst) - hits_after_r1, 1);

        store.delete_region(r1.id).await.expect("delete r1");
        let remaining = store.list_regions().await.expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, r2.id);
        assert!(
            store
                .get_resource(&format!("{prefix}/tiles/0/0/0.pbf"))
                .await
                .expect("get shared z0")
                .is_some(),
            "shared z=0 tile must survive deleting r1 because r2 still pins it"
        );

        // 2. `invalidate_region` clears completion and unlinks resources cleanly.
        store.invalidate_region(r2.id).await.expect("invalidate r2");
        let inv_status = store.region_status(r2.id).await.expect("status");
        assert!(!inv_status.complete);
        assert_eq!(inv_status.completed_tile_count, 0);
        store.delete_region(r2.id).await.expect("delete r2");

        // 3. `set_tile_count_limit` rejects downloads exceeding the tile ceiling.
        store
            .set_tile_count_limit(Some(2))
            .await
            .expect("set limit");
        let over_limit_def =
            OfflineRegionDefinition::from_style_url(&style_url, [-180.0, -85.0, 180.0, 85.0], 0, 1);
        let limit_err = manager
            .download_region(over_limit_def, OfflineCancellationToken::new(), |_| {})
            .await
            .expect_err("5 tiles > limit 2 must fail");
        assert!(matches!(
            limit_err,
            OfflineDownloadError::Store(OfflineStoreError::TileLimitExceeded { limit: 2 })
        ));
        store.set_tile_count_limit(None).await.expect("clear limit");

        // 4. `set_maximum_ambient_bytes` evicts oldest unpinned ambient entries.
        store
            .put_ambient_resource("https://ambient.test/1", ResourceKind::Tile, &[1_u8; 64])
            .await
            .expect("amb 1");
        store
            .put_ambient_resource("https://ambient.test/2", ResourceKind::Tile, &[2_u8; 64])
            .await
            .expect("amb 2");
        store
            .set_maximum_ambient_bytes(80)
            .await
            .expect("shrink ambient budget");
        assert!(store
            .get_resource("https://ambient.test/1")
            .await
            .expect("q1")
            .is_none());
        assert!(store
            .get_resource("https://ambient.test/2")
            .await
            .expect("q2")
            .is_some());

        // 5. Cooperative cancellation via `OfflineCancellationToken`.
        let cancelled = OfflineCancellationToken::new();
        cancelled.cancel();
        let cancel_err = manager
            .download_region(
                OfflineRegionDefinition::from_style_url(&style_url, [0.0, 0.0, 1.0, 1.0], 0, 1),
                cancelled,
                |_| {},
            )
            .await
            .expect_err("pre-cancelled token must abort");
        assert!(matches!(cancel_err, OfflineDownloadError::Cancelled(_)));

        // 6. `merge_database` imports a sideloaded SQLite pack and serves its resources offline.
        {
            let side_store = Arc::new(SqliteOfflineStore::open(&side_path).expect("side db"));
            let side_mgr = OfflineManager::new(
                SourceClient::new(HttpSourceClient::new(http)),
                Arc::clone(&side_store),
            );
            let side_def = OfflineRegionDefinition::from_style_url(
                &style_url,
                [139.7, 35.6, 139.8, 35.7],
                0,
                1,
            )
            .with_metadata(b"tokyo-sideload".to_vec());
            side_mgr
                .download_region(side_def, OfflineCancellationToken::new(), |_| {})
                .await
                .expect("populate sideload db");
        }
        let merged_ids = store
            .merge_database(&side_path)
            .await
            .expect("merge sideloaded sqlite db");
        assert_eq!(merged_ids.len(), 1);
        let merged_def = store
            .region_definition(merged_ids[0])
            .await
            .expect("merged definition");
        assert_eq!(merged_def.metadata, b"tokyo-sideload");
        let merged_status = store
            .region_status(merged_ids[0])
            .await
            .expect("merged status");
        assert!(merged_status.complete);
        assert_eq!(merged_status.completed_tile_count, 2);

        let _ = std::fs::remove_file(&main_path);
        let _ = std::fs::remove_file(&side_path);
    }
}

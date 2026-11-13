#![allow(clippy::expect_used, clippy::panic)]

use std::io::ErrorKind;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;

#[derive(Debug, thiserror::Error)]
#[error("middleware transport wrapper: {0}")]
struct TransportWrapper(#[source] std::io::Error);

#[derive(Debug, thiserror::Error)]
#[error("cache metadata corrupt")]
struct CacheMetadataCorrupt;

#[test]
fn middleware_wrapping_transient_io_errors_is_retryable() {
    for io_err in [
        std::io::Error::new(ErrorKind::TimedOut, "socket timed out"),
        std::io::Error::new(ErrorKind::ConnectionRefused, "connection refused"),
        std::io::Error::new(ErrorKind::ConnectionReset, "connection reset by peer"),
        std::io::Error::new(ErrorKind::NetworkUnreachable, "network unreachable"),
        std::io::Error::new(ErrorKind::HostUnreachable, "no route to host"),
        std::io::Error::other(
            "failed to lookup address information: nodename nor servname provided, or not known",
        ),
    ] {
        let mw_err = reqwest_middleware::Error::middleware(TransportWrapper(io_err));
        let fetch_err = SourceFetchError::from(mw_err);
        assert!(
            fetch_err.is_retryable(),
            "expected retryable for {}",
            fetch_err.describe()
        );
    }
}

#[test]
fn non_network_middleware_errors_stay_non_retryable() {
    for mw_err in [
        reqwest_middleware::Error::middleware(CacheMetadataCorrupt),
        reqwest_middleware::Error::middleware(std::io::Error::new(
            ErrorKind::InvalidData,
            "invalid cache entry",
        )),
        reqwest_middleware::Error::middleware(std::io::Error::other(
            "unexpected header serialization error",
        )),
    ] {
        let fetch_err = SourceFetchError::from(mw_err);
        assert!(
            !fetch_err.is_retryable(),
            "expected non-retryable for {}",
            fetch_err.describe()
        );
    }
}

#[tokio::test]
async fn cached_client_connect_or_dns_reqwest_error_in_middleware_is_retryable() {
    let raw_client = Client::builder()
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .expect("reqwest client");
    let connect_err = raw_client
        .get("http://127.0.0.1:1/tiles/0/0/0.png")
        .send()
        .await
        .expect_err("closed local port must fail");

    let direct_mw = SourceFetchError::from(reqwest_middleware::Error::middleware(connect_err));
    assert!(
        direct_mw.is_retryable(),
        "direct middleware reqwest connect error must be retryable: {}",
        direct_mw.describe()
    );

    let cache_dir =
        std::env::temp_dir().join(format!("maplibre-http-cache-test-{}", std::process::id()));
    let cached_client = ReqwestHttpClient::new(Some(&cache_dir));
    let cached_err = cached_client
        .fetch("http://127.0.0.1:1/tiles/0/0/0.png")
        .await
        .expect_err("cached fetch against closed port must fail");
    let _ = std::fs::remove_dir_all(&cache_dir);
    assert!(
        cached_err.is_retryable(),
        "http-cache-reqwest wrapped transport error must be retryable: {}",
        cached_err.describe()
    );

    let bad_url_err = raw_client
        .get("://not-a-valid-url")
        .send()
        .await
        .expect_err("malformed URL must fail");
    let bad_url_mw = SourceFetchError::from(reqwest_middleware::Error::middleware(bad_url_err));
    assert!(
        !bad_url_mw.is_retryable(),
        "malformed URL builder error must remain terminal: {}",
        bad_url_mw.describe()
    );
}

#[test]
fn memory_tile_cache_evicts_least_recently_used_entries_within_budget() {
    let mut cache = MemoryTileCacheState::new(10);
    cache.insert("a", &[1, 2, 3, 4]);
    cache.insert("b", &[5, 6, 7, 8]);
    assert_eq!(cache.get("a").as_deref(), Some(&[1, 2, 3, 4][..]));
    cache.insert("c", &[9, 10, 11, 12]);
    assert_eq!(cache.get("a").as_deref(), Some(&[1, 2, 3, 4][..]));
    assert!(cache.get("b").is_none());
    assert_eq!(cache.get("c").as_deref(), Some(&[9, 10, 11, 12][..]));
    assert!(cache.bytes <= 10);
}

#[tokio::test]
async fn cached_range_requests_request_identity_encoding_and_preserve_distinct_slices() {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
        // Sandboxed test runners without loopback socket bind permission skip wire check.
        return;
    };
    let addr = listener.local_addr().expect("local addr");
    let archive_bytes: Arc<[u8]> = Arc::from(&b"PMTiles\x03ROOT_DIRECTORY_PAYLOAD_BYTES"[..]);
    let server_bytes = Arc::clone(&archive_bytes);

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut req_buf = vec![0_u8; 4096];
            let n = stream.read(&mut req_buf).await.expect("read request");
            let req_str = String::from_utf8_lossy(&req_buf[..n]).to_ascii_lowercase();

            // Simulate Fastly / GitHub Pages dynamic gzip corruption on Range requests when
            // `Accept-Encoding: gzip` is advertised instead of `identity`.
            assert!(
                req_str.contains("accept-encoding: identity") && !req_str.contains("gzip"),
                "range request must advertise identity encoding only, got:\n{req_str}"
            );

            let (start, end) = req_str
                .lines()
                .find_map(|line| line.strip_prefix("range: bytes="))
                .and_then(|spec| spec.trim().split_once('-'))
                .map(|(s, e)| {
                    (
                        s.parse::<usize>().expect("start"),
                        e.parse::<usize>().expect("end"),
                    )
                })
                .expect("Range header present");
            let slice = &server_bytes[start..=end];
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Cache-Control: public, max-age=600\r\n\
                 Accept-Ranges: bytes\r\n\
                 Content-Range: bytes {start}-{end}/{total}\r\n\
                 Content-Length: {len}\r\n\
                 Connection: close\r\n\r\n",
                total = server_bytes.len(),
                len = slice.len(),
            );
            stream.write_all(header.as_bytes()).await.expect("header");
            stream.write_all(slice).await.expect("body");
        }
    });

    let cache_dir = std::env::temp_dir().join(format!(
        "maplibre-http-range-cache-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let client = ReqwestHttpClient::new(Some(&cache_dir));
    let url = format!("http://{addr}/archive.pmtiles");

    let first = client
        .fetch_range(
            &url,
            ByteRange {
                offset: 0,
                length: 8,
            },
        )
        .await
        .expect("header range");
    assert_eq!(first, b"PMTiles\x03");

    let second = client
        .fetch_range(
            &url,
            ByteRange {
                offset: 8,
                length: 14,
            },
        )
        .await
        .expect("directory range must not collide with cached header range");
    assert_eq!(second, b"ROOT_DIRECTORY");

    // Repeated range hits the in-memory range cache without opening a 3rd TCP connection.
    let cached_first = client
        .fetch_range(
            &url,
            ByteRange {
                offset: 0,
                length: 8,
            },
        )
        .await
        .expect("in-memory cached header range");
    assert_eq!(cached_first, b"PMTiles\x03");

    server
        .await
        .expect("server task cleanly handled 2 requests");
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[tokio::test]
async fn in_process_routes_are_isolated_per_client_and_shared_frontend_hits_memory_cache() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = Arc::new(AtomicUsize::new(0));
    let route_calls = Arc::clone(&calls);
    let routes = InProcessRoutes::new();
    let prefix = "http://127.0.0.1:1/in-process-isolated";
    routes.register(
        prefix,
        Arc::new(move |_url, _range| {
            let count = route_calls.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move { Ok(format!("reply-{count}").into_bytes()) })
        }),
    );

    let routed = ReqwestHttpClient::with_in_process_routes::<String>(None, routes.clone());
    let plain = ReqwestHttpClient::new::<String>(None);
    let target = format!("{prefix}/tile.pbf");

    assert_eq!(routed.fetch(&target).await.expect("routed"), b"reply-1");
    assert!(plain.fetch(&target).await.is_err());

    routes.unregister(prefix);
    assert!(routed.fetch(&target).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let frontend = HttpCacheFrontend::new();
    let first = ReqwestHttpClient::with_cache_frontend::<String>(None, frontend.clone());
    let second = ReqwestHttpClient::with_cache_frontend::<String>(None, frontend);
    first
        .frontend
        .as_deref()
        .expect("frontend attached")
        .insert("http://127.0.0.1:1/shared", b"cached-payload");
    assert_eq!(
        second
            .fetch("http://127.0.0.1:1/shared")
            .await
            .expect("served from shared frontend"),
        b"cached-payload"
    );
}

#[tokio::test]
async fn volatile_policy_bypasses_disk_and_memory_caches_while_offline_serves_cached_hits() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::io::source_client::{HttpSourceClient, NetworkState};

    let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
        return;
    };
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = Arc::clone(&hits);

    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let n_hit = server_hits.fetch_add(1, Ordering::SeqCst) + 1;
            let mut buf = vec![0_u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req_text = String::from_utf8_lossy(&buf[..n]);
            let first_line = req_text.lines().next().unwrap_or_default();
            let body = format!("payload-{n_hit}:{first_line}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Cache-Control: public, max-age=3600\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let cache_dir = std::env::temp_dir().join(format!(
        "maplibre-volatile-offline-cache-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache_dir);

    let normal_url = format!("http://{addr}/normal/0/0/0.pbf");
    let volatile_url = format!("http://{addr}/volatile/0/0/0.pbf");
    let uncached_url = format!("http://{addr}/uncached/0/0/0.pbf");

    // Session 1: populate normal URL in disk cache; fetch volatile URL twice.
    {
        let client1 = HttpSourceClient::new(ReqwestHttpClient::new(Some(&cache_dir)));
        let normal_1 = client1
            .fetch_source_url(&normal_url, false)
            .await
            .expect("normal online fetch");
        assert!(normal_1.starts_with(b"payload-1:"));

        let vol_1 = client1
            .fetch_source_url(&volatile_url, true)
            .await
            .expect("volatile fetch 1");
        let vol_2 = client1
            .fetch_source_url(&volatile_url, true)
            .await
            .expect("volatile fetch 2 must bypass memory & disk cache");
        assert_ne!(
            vol_1, vol_2,
            "volatile source must re-hit origin rather than serving cached response"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    // Session 2 (fresh client sharing `cache_dir`, initially Offline):
    let net = NetworkState::offline();
    let client2 = HttpSourceClient::new(ReqwestHttpClient::new(Some(&cache_dir)))
        .with_network_state(net.clone());

    // Normal URL succeeds from persistent `CACacheManager` on disk with ZERO new network sockets!
    let cached_offline = client2
        .fetch_source_url(&normal_url, false)
        .await
        .expect("normal cached URL must load from disk while offline");
    assert!(cached_offline.starts_with(b"payload-1:"));
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // Volatile URL was never persisted to disk, so offline fetch fails with `OfflineError`.
    let vol_offline_err = client2
        .fetch_source_url(&volatile_url, true)
        .await
        .expect_err("volatile URL must not be in persistent disk cache");
    assert!(vol_offline_err.is_offline());
    assert!(vol_offline_err.is_retryable());
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // Uncached remote URL fails immediately with `OfflineError` and opens zero TCP connections.
    let uncached_err = client2
        .fetch_url(&uncached_url)
        .await
        .expect_err("uncached URL must fail while offline");
    assert!(uncached_err.is_offline());
    assert!(uncached_err.is_retryable());
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // Flipping `set_online(true)` recovers immediately without reconstructing clients.
    net.set_online(true);
    let recovered = client2
        .fetch_url(&uncached_url)
        .await
        .expect("online recovery must fetch uncached URL");
    assert!(recovered.starts_with(b"payload-4:"));
    assert_eq!(hits.load(Ordering::SeqCst), 4);

    server.abort();
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[tokio::test]
async fn request_transform_rewrites_custom_scheme_and_injects_headers_on_wire() {
    use crate::io::source_client::{HttpSourceClient, ResourceKind};

    let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
        return;
    };
    let addr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 4096];
        let n = stream.read(&mut buf).await.expect("read");
        let req_text = String::from_utf8_lossy(&buf[..n]);
        let lower = req_text.to_ascii_lowercase();
        assert!(
            req_text.starts_with("GET /v4/mapbox.streets.json?access_token=pk.test HTTP/1.1"),
            "unexpected request line: {req_text}"
        );
        assert!(
            lower.contains("authorization: bearer secret-jwt"),
            "missing Authorization header: {req_text}"
        );
        assert!(
            lower.contains("x-api-key: key-123"),
            "missing X-Api-Key header: {req_text}"
        );
        let body = br#"{"tiles":["https://example.test/{z}/{x}/{y}.pbf"]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes()).await.expect("write hdr");
        stream.write_all(body).await.expect("write body");
    });

    let base_client = HttpSourceClient::new(ReqwestHttpClient::new::<String>(None));
    let cloned_client = base_client.clone();

    // Install transform on `base_client`; `cloned_client` shares the same `Arc<RwLock<...>>` slot.
    base_client.set_request_transform(move |mut req| {
        if let Some(rest) = req.url.strip_prefix("mapbox://") {
            req.url = format!("http://{addr}/v4/{rest}.json?access_token=pk.test");
        }
        if req.resource_kind == ResourceKind::Source {
            req.headers
                .push(("Authorization".to_owned(), "Bearer secret-jwt".to_owned()));
            req.headers
                .push(("X-Api-Key".to_owned(), "key-123".to_owned()));
        }
        req
    });

    let body = cloned_client
        .fetch_source_url("mapbox://mapbox.streets", false)
        .await
        .expect("transformed request succeeds");
    assert!(body.starts_with(b"{\"tiles\":"));

    server.await.expect("server verified headers and path");
}

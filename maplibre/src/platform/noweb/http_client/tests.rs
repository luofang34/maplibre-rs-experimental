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

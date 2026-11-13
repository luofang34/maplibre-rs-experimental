#![allow(clippy::expect_used, clippy::panic)]

use super::SourceFetchError;

#[test]
fn a_missing_tile_is_told_apart_from_other_failures() {
    let not_found = SourceFetchError::not_found("https://example.test/1/0/0.pbf");
    let refused = SourceFetchError(Box::new(std::io::Error::other("connection refused")));

    assert!(not_found.is_not_found());
    assert!(!refused.is_not_found());
}

#[test]
fn the_description_keeps_the_cause_chain() {
    let refused = SourceFetchError(Box::new(std::io::Error::other("connection refused")));

    assert_eq!(
        refused.describe(),
        "failed to fetch from source: connection refused"
    );
}

#[test]
fn retry_classification_preserves_status_and_original_failure_context() {
    for status in [400, 401, 403, 404, 408, 410, 429, 500, 503] {
        let error = SourceFetchError::http_response(
            "https://tiles.test/a",
            status,
            std::io::Error::other("transport response detail"),
        );
        assert_eq!(
            error.is_retryable(),
            matches!(status, 408 | 429 | 500 | 503)
        );
        assert!(error.describe().contains("https://tiles.test/a"));
        assert!(error.describe().contains(&status.to_string()));
        assert!(error.describe().contains("transport response detail"));
    }
    let temporary = SourceFetchError::temporary(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "DNS timeout",
    ));
    assert!(temporary.is_retryable());
    assert!(temporary.describe().contains("DNS timeout"));
    assert!(!SourceFetchError::not_found("https://tiles.test/a").is_retryable());
    assert!(!SourceFetchError(Box::new(std::io::Error::other("unknown failure"))).is_retryable());
}

#[derive(Debug, thiserror::Error)]
#[error("Middleware error: {0}")]
struct OuterMiddlewareError(#[source] InnerRequestError);

#[derive(Debug, thiserror::Error)]
#[error("Request error: {0}")]
struct InnerRequestError(#[source] LeafTransportError);

#[derive(Debug, thiserror::Error)]
#[error("error sending request for url (https://tiles.test/0/0/0.png)")]
struct LeafTransportError(#[source] std::io::Error);

#[test]
fn describe_deduplicates_nested_messages_already_embedded_in_outer_display() {
    let leaf = LeafTransportError(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "dns lookup timed out",
    ));
    let nested = OuterMiddlewareError(InnerRequestError(leaf));
    let error = SourceFetchError::temporary(nested);

    let description = error.describe();
    assert_eq!(
        description
            .matches("error sending request for url (https://tiles.test/0/0/0.png)")
            .count(),
        1,
        "unexpected duplicate cause in description: {description}"
    );
    assert_eq!(description.matches("Request error:").count(), 1);
    assert!(
        description.contains("dns lookup timed out"),
        "{description}"
    );
}

#[tokio::test]
async fn data_urls_and_local_files_succeed_while_network_state_is_offline() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use async_trait::async_trait;

    use super::{
        ByteRange, HttpClient, HttpRequest, HttpSourceClient, NetworkState, NetworkStatus,
        ResourceKind,
    };
    use crate::io::resource_loader::SharedLoader;

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

    // Inline GeoJSON / data: URLs (both plain percent-encoded and base64) work while offline.
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

    // Local `file://` URLs also succeed while offline.
    let temp_file = std::env::temp_dir().join(format!(
        "maplibre-offline-file-{}.geojson",
        std::process::id()
    ));
    std::fs::write(&temp_file, br#"{"type":"Point","coordinates":[0,0]}"#).expect("write temp");
    let file_bytes = client
        .fetch_url(&format!("file://{}", temp_file.display()))
        .await
        .expect("file:// URL while offline");
    let _ = std::fs::remove_file(&temp_file);
    assert_eq!(file_bytes, br#"{"type":"Point","coordinates":[0,0]}"#);

    // Remote HTTP miss fails immediately with `OfflineError` (retryable).
    let remote_err = client
        .fetch_url("https://tiles.example.test/1/0/0.pbf")
        .await
        .expect_err("remote HTTP must fail while offline");
    assert!(remote_err.is_offline());
    assert!(remote_err.is_retryable());
}

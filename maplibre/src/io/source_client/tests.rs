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

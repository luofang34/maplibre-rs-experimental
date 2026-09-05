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

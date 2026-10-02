use std::sync::{Arc, Mutex};

use super::*;

/// Serves the bytes 0..=255 at every URL and records what was asked.
#[derive(Clone, Default)]
struct Bytes {
    asked: Arc<Mutex<Vec<String>>>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for Bytes {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.asked.lock().expect("asked").push(url.to_owned());
        Ok((0..=255).collect())
    }
}

#[tokio::test]
async fn a_transport_without_ranges_cuts_the_range_out_of_the_whole_body() {
    let bytes = Bytes::default();
    let loader = SharedLoader::new(bytes.clone());
    let range = ByteRange {
        offset: 10,
        length: 4,
    };
    assert_eq!(
        loader
            .fetch_range("https://a.invalid/x", range)
            .await
            .expect("range"),
        vec![10, 11, 12, 13]
    );
    assert_eq!(range.header(), "bytes=10-13");
    let past = ByteRange {
        offset: 250,
        length: 10,
    };
    let error = loader
        .fetch_range("https://a.invalid/x", past)
        .await
        .expect_err("past the end");
    assert!(
        error.describe().contains("past the end"),
        "{}",
        error.describe()
    );
    assert_eq!(bytes.asked.lock().expect("asked").len(), 2);
}

#[tokio::test]
async fn clones_of_a_shared_loader_reach_the_same_loader() {
    let bytes = Bytes::default();
    let loader = SharedLoader::new(bytes.clone());
    let clone = loader.clone();
    loader.fetch("https://a.invalid/1").await.expect("first");
    clone.fetch("https://a.invalid/2").await.expect("second");
    assert_eq!(
        *bytes.asked.lock().expect("asked"),
        vec!["https://a.invalid/1", "https://a.invalid/2"]
    );
}

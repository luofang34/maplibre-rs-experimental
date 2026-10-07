use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use super::*;
use crate::io::source_client::{HttpClient, SourceFetchError};

/// Counts the requests it answers.
#[derive(Clone, Default)]
struct Counting(Arc<AtomicUsize>);

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Counting {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![1, 2, 3])
    }
}

#[tokio::test]
async fn workers_fetch_through_the_configured_loader() {
    let counting = Counting::default();
    let kernel = LoaderKernel::create(OffscreenKernelConfig {
        loader: Some(SharedLoader::new(counting.clone())),
        ..Default::default()
    });

    let body = kernel
        .source_client()
        .fetch_url("https://tiles.example/0/0/0.pbf")
        .await
        .expect("the configured loader answers");

    assert_eq!(body.as_ref(), [1, 2, 3]);
    assert_eq!(counting.0.load(Ordering::SeqCst), 1);
}

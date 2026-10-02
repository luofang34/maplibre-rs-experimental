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

/// Holds every fetch until released, counting fetches and fetches dropped before finishing.
#[derive(Clone, Default)]
struct Gated {
    fetches: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
    entered: Arc<tokio::sync::Notify>,
}

struct CountDrop(Arc<std::sync::atomic::AtomicUsize>, bool);
impl Drop for CountDrop {
    fn drop(&mut self) {
        if !self.1 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl HttpClient for Gated {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut guard = CountDrop(self.dropped.clone(), false);
        self.entered.notify_one();
        self.release.notified().await;
        guard.1 = true;
        if url.ends_with("missing") {
            return Err(SourceFetchError::not_found(url));
        }
        Ok(url.as_bytes().to_vec())
    }
}

fn spawn_fetch(
    loader: &SharedLoader,
    url: &'static str,
) -> tokio::task::JoinHandle<Result<Vec<u8>, SourceFetchError>> {
    let loader = loader.clone();
    tokio::task::spawn_local(async move { loader.fetch(url).await })
}

#[tokio::test]
async fn concurrent_requests_for_the_same_bytes_fetch_once() {
    use std::sync::atomic::Ordering;
    tokio::task::LocalSet::new()
        .run_until(async {
            let gated = Gated::default();
            let loader = SharedLoader::new(gated.clone());
            let first = spawn_fetch(&loader, "https://a.invalid/t");
            gated.entered.notified().await;
            let second = spawn_fetch(&loader, "https://a.invalid/t");
            let other = spawn_fetch(&loader, "https://a.invalid/u");
            gated.entered.notified().await;
            gated.release.notify_waiters();
            let first = first.await.expect("task").expect("first");
            let second = second.await.expect("task").expect("second");
            let other = other.await.expect("task").expect("other");
            assert_eq!(first, second);
            assert_eq!(other, b"https://a.invalid/u");
            assert_eq!(
                gated.fetches.load(Ordering::SeqCst),
                2,
                "one fetch per distinct resource"
            );
            // A request after the shared one finished fetches anew.
            let again = spawn_fetch(&loader, "https://a.invalid/t");
            gated.entered.notified().await;
            gated.release.notify_waiters();
            again.await.expect("task").expect("again");
            assert_eq!(gated.fetches.load(Ordering::SeqCst), 3);
        })
        .await;
}

#[tokio::test]
async fn a_waiter_fetches_itself_when_the_request_it_waits_for_is_cancelled() {
    use std::sync::atomic::Ordering;
    tokio::task::LocalSet::new()
        .run_until(async {
            let gated = Gated::default();
            let loader = SharedLoader::new(gated.clone());
            let leader = spawn_fetch(&loader, "https://a.invalid/t");
            gated.entered.notified().await;
            let waiter = spawn_fetch(&loader, "https://a.invalid/t");
            tokio::task::yield_now().await;
            leader.abort();
            assert!(leader.await.is_err(), "the leader was cancelled");
            assert_eq!(gated.dropped.load(Ordering::SeqCst), 1, "its fetch stopped");
            gated.entered.notified().await;
            gated.release.notify_waiters();
            assert_eq!(
                waiter.await.expect("task").expect("waiter"),
                b"https://a.invalid/t"
            );
            assert_eq!(gated.fetches.load(Ordering::SeqCst), 2);
        })
        .await;
}

#[tokio::test]
async fn a_waiter_gets_the_failure_classified_as_the_fetch_was() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let gated = Gated::default();
            let loader = SharedLoader::new(gated.clone());
            let leader = spawn_fetch(&loader, "https://a.invalid/missing");
            gated.entered.notified().await;
            let waiter = spawn_fetch(&loader, "https://a.invalid/missing");
            tokio::task::yield_now().await;
            gated.release.notify_waiters();
            assert!(leader
                .await
                .expect("task")
                .expect_err("missing")
                .is_not_found());
            assert!(waiter
                .await
                .expect("task")
                .expect_err("missing")
                .is_not_found());
        })
        .await;
}

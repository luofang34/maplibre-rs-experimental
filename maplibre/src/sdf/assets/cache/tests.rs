#![allow(clippy::expect_used, clippy::panic)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use super::*;
use crate::io::source_client::HttpSourceClient;

const BUNDLED: &[u8] = include_bytes!("../../../../../data/0-255.pbf");

enum Step {
    Ok,
    NotFound,
    Temporary,
    Hang,
}

/// Answers every request with a valid glyph range unless a scripted step says otherwise.
#[derive(Clone, Default)]
struct Scripted {
    calls: Arc<Mutex<Vec<String>>>,
    script: Arc<Mutex<VecDeque<Step>>>,
}

impl Scripted {
    fn then(&self, step: Step) -> &Self {
        self.script.lock().expect("script").push_back(step);
        self
    }

    fn calls(&self) -> usize {
        self.calls.lock().expect("calls").len()
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Scripted {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.calls.lock().expect("calls").push(url.to_owned());
        let step = self.script.lock().expect("script").pop_front();
        // Let concurrent requests reach the cache before this one completes.
        tokio::task::yield_now().await;
        match step.unwrap_or(Step::Ok) {
            Step::Ok => Ok(BUNDLED.to_vec()),
            Step::NotFound => Err(SourceFetchError::not_found(url)),
            Step::Temporary => Err(SourceFetchError::temporary(std::io::Error::other("reset"))),
            Step::Hang => std::future::pending().await,
        }
    }
}

fn client(http: &Scripted) -> SourceClient<Scripted> {
    SourceClient::new(HttpSourceClient::new(http.clone()))
}

const RANGE: &str = "https://fonts.invalid/a/0-255.pbf";

#[tokio::test]
async fn a_second_tile_reuses_the_decoded_range() {
    let http = Scripted::default();
    let (client, cache) = (client(&http), AssetCache::default());
    let first = cache.glyphs(&client, RANGE).await.expect("first");
    let second = cache.glyphs(&client, RANGE).await.expect("second");
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(http.calls(), 1);
    assert!(cache.approximate_bytes() > 0);
}

#[tokio::test]
async fn concurrent_tiles_wait_for_one_fetch() {
    let http = Scripted::default();
    let (client, cache) = (client(&http), AssetCache::default());
    let requests = (0..8).map(|_| cache.glyphs(&client, RANGE));
    let results = futures_join(requests).await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(http.calls(), 1);
}

#[tokio::test]
async fn a_missing_range_is_not_requested_again() {
    let http = Scripted::default();
    http.then(Step::NotFound);
    let (client, cache) = (client(&http), AssetCache::default());
    for _ in 0..3 {
        assert_eq!(
            cache.glyphs(&client, RANGE).await.err(),
            Some(AssetFailure::NotFound)
        );
    }
    assert_eq!(http.calls(), 1);
}

#[tokio::test]
async fn a_transient_failure_reaches_every_waiter_once_and_is_not_remembered() {
    let http = Scripted::default();
    http.then(Step::Temporary);
    let (client, cache) = (client(&http), AssetCache::default());
    let failures = futures_join((0..4).map(|_| cache.glyphs(&client, RANGE))).await;
    assert!(failures
        .iter()
        .all(|result| matches!(result, Err(AssetFailure::Retryable(_)))));
    assert_eq!(http.calls(), 1);
    cache.glyphs(&client, RANGE).await.expect("retry succeeds");
    assert_eq!(http.calls(), 2);
}

#[tokio::test]
async fn least_recently_used_ranges_are_evicted_under_the_byte_budget() {
    let http = Scripted::default();
    let (client, cache) = (client(&http), AssetCache::with_budget(1));
    let other = "https://fonts.invalid/b/0-255.pbf";
    cache.glyphs(&client, RANGE).await.expect("a");
    cache.glyphs(&client, other).await.expect("b");
    let held = cache.approximate_bytes();
    cache.glyphs(&client, RANGE).await.expect("a again");
    assert_eq!(http.calls(), 3, "the evicted range is fetched again");
    assert_eq!(cache.approximate_bytes(), held, "one range stays resident");
}

#[tokio::test]
async fn a_cancelled_fetch_lets_the_next_request_load_the_range() {
    let http = Scripted::default();
    http.then(Step::Hang);
    let (client, cache) = (client(&http), AssetCache::default());
    let stalled = tokio::time::timeout(Duration::from_millis(20), cache.glyphs(&client, RANGE));
    assert!(stalled.await.is_err(), "the first fetch never completes");
    cache.glyphs(&client, RANGE).await.expect("takes over");
    assert_eq!(http.calls(), 2);
}

#[tokio::test]
async fn a_waiter_takes_over_when_the_leading_fetch_is_cancelled() {
    let http = Scripted::default();
    http.then(Step::Hang);
    let (client, cache) = (client(&http), AssetCache::default());
    let leader = cache.glyphs(&client, RANGE);
    let waiter = cache.glyphs(&client, RANGE);
    let cancelled = tokio::time::timeout(Duration::from_millis(20), leader);
    let (leader, waiter) = tokio::join!(cancelled, waiter);
    assert!(leader.is_err());
    waiter.expect("waiter loads it itself");
    assert_eq!(http.calls(), 2);
}

#[tokio::test]
async fn an_undecodable_range_is_terminal_and_remembered() {
    #[derive(Clone)]
    struct Garbage(Arc<Mutex<usize>>);
    #[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
    #[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
    impl HttpClient for Garbage {
        async fn fetch(&self, _url: &str) -> Result<Vec<u8>, SourceFetchError> {
            *self.0.lock().expect("count") += 1;
            Ok(vec![0xff; 16])
        }
    }
    let http = Garbage(Arc::default());
    let client = SourceClient::new(HttpSourceClient::new(http.clone()));
    let cache = AssetCache::default();
    for _ in 0..2 {
        assert!(matches!(
            cache.glyphs(&client, RANGE).await,
            Err(AssetFailure::Terminal(_))
        ));
    }
    assert_eq!(*http.0.lock().expect("count"), 1);
}

async fn futures_join<T>(
    futures: impl Iterator<Item = impl std::future::Future<Output = T>>,
) -> Vec<T> {
    let handles: Vec<_> = futures.map(Box::pin).collect();
    let mut results = Vec::new();
    let mut pending: Vec<_> = handles.into_iter().map(Some).collect();
    let mut slots: Vec<Option<T>> = pending.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut done = true;
        for (index, future) in pending.iter_mut().enumerate() {
            if let Some(inner) = future {
                match inner.as_mut().poll(context) {
                    std::task::Poll::Ready(value) => {
                        slots[index] = Some(value);
                        *future = None;
                    }
                    std::task::Poll::Pending => done = false,
                }
            }
        }
        if done {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    results.extend(slots.into_iter().flatten());
    results
}

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use futures::FutureExt as _;

use super::{
    ImageProviderError, ImageProviders, ImageRequest, ImageResolution, ProvideFuture,
    ProvidedImage, ProviderLimits, Resolved, StyleImageProvider,
};
use crate::style::StyleImage;

/// What the fake answers next.
#[derive(Clone, Debug)]
enum Answer {
    Image,
    Absent,
    Failed,
    Unavailable,
    Malformed,
}

/// Answers every request alike, counting calls, optionally once a gate opens.
struct Fake {
    calls: AtomicUsize,
    requests: Mutex<Vec<ImageRequest>>,
    answer: Mutex<Answer>,
    generation: Mutex<String>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
}

impl Fake {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::default(),
            answer: Mutex::new(answer),
            generation: Mutex::new("pack-1".into()),
            gate: None,
        })
    }

    fn held() -> (Arc<Self>, Arc<tokio::sync::Semaphore>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let fake = Arc::new(Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::default(),
            answer: Mutex::new(Answer::Image),
            generation: Mutex::new("pack-1".into()),
            gate: Some(gate.clone()),
        });
        (fake, gate)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl StyleImageProvider for Fake {
    fn generation(&self) -> String {
        self.generation.lock().expect("generation").clone()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.acquire().await.expect("gate").forget();
            }
            let size = (2.0 * request.pixel_ratio) as u32;
            self.requests.lock().expect("requests").push(request);
            let image = |data_len: usize| {
                ImageResolution::Image(ProvidedImage {
                    image: StyleImage {
                        width: size,
                        height: size,
                        data: vec![255; data_len],
                        pixel_ratio: 2.0,
                        sdf: false,
                    },
                    anchor: None,
                })
            };
            match self.answer.lock().expect("answer").clone() {
                Answer::Image => Ok(image((size * size * 4) as usize)),
                Answer::Malformed => Ok(image(3)),
                Answer::Absent => Ok(ImageResolution::Absent),
                Answer::Failed => Err(ImageProviderError::Failed("bad route".into())),
                Answer::Unavailable => Err(ImageProviderError::Unavailable("pack loading".into())),
            }
        })
    }
}

fn registry(provider: Arc<Fake>, limits: ProviderLimits) -> ImageProviders {
    let providers = ImageProviders::with_limits(limits);
    providers.register("shield", provider);
    providers
}

fn is_image(answer: &Result<Arc<Resolved>, String>) -> bool {
    matches!(answer.as_deref(), Ok(Resolved::Image(_)))
}

/// Polls `waiting` until the registry reports `running` generators, then opens `gate`.
async fn open_when_running(
    providers: &ImageProviders,
    running: usize,
    gate: &tokio::sync::Semaphore,
) {
    while providers.stats().running < running {
        tokio::task::yield_now().await;
    }
    gate.add_permits(64);
}

#[tokio::test]
async fn many_tiles_asking_for_one_name_call_its_provider_once() {
    let (fake, gate) = Fake::held();
    let providers = registry(fake.clone(), ProviderLimits::default());
    let asks = (0..8).map(|_| providers.resolve("shield:US:I=287", 2.0));
    let (answers, ()) = futures::join!(
        futures::future::join_all(asks),
        open_when_running(&providers, 1, &gate)
    );
    assert!(answers.iter().all(is_image), "{answers:?}");
    assert_eq!(fake.calls(), 1, "concurrent requests share one call");
    let request = fake.requests.lock().expect("requests")[0].clone();
    assert_eq!(request.id, "US:I=287");
    assert_eq!(request.name, "shield:US:I=287");
    assert_eq!(request.pixel_ratio, 2.0);
    assert!(providers.known("shield:US:I=287", 2.0).is_some());
    assert_eq!(fake.calls(), 1, "a known answer is not asked again");
}

#[tokio::test]
async fn names_of_other_namespaces_are_not_provided() {
    let fake = Fake::new(Answer::Image);
    let providers = registry(fake.clone(), ProviderLimits::default());
    assert!(providers.provides("shield:1"));
    assert!(!providers.provides("shields:1"));
    assert!(!providers.provides("marker"));
    assert!(providers.known("marker", 1.0).is_none());
    assert!(matches!(
        providers.resolve("marker", 1.0).await.as_deref(),
        Ok(Resolved::None)
    ));
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn a_full_queue_refuses_more_requests_for_now() {
    let (fake, gate) = Fake::held();
    let limits = ProviderLimits {
        max_running: 1,
        max_waiting: 1,
        ..ProviderLimits::default()
    };
    let providers = registry(fake.clone(), limits);
    let asks = ["shield:1", "shield:2", "shield:3"].map(|name| providers.resolve(name, 1.0));
    let opener = async {
        while providers.stats().refused == 0 {
            tokio::task::yield_now().await;
        }
        let stats = providers.stats();
        assert_eq!((stats.running, stats.waiting), (1, 1), "{stats:?}");
        gate.add_permits(64);
    };
    let (answers, ()) = futures::join!(futures::future::join_all(asks), opener);
    assert_eq!(answers.iter().filter(|answer| is_image(answer)).count(), 2);
    assert_eq!(
        answers.iter().filter(|answer| answer.is_err()).count(),
        1,
        "{answers:?}"
    );
    let stats = providers.stats();
    assert_eq!(
        (stats.refused, stats.calls, stats.running, stats.waiting),
        (1, 2, 0, 0)
    );
    // A refusal is not remembered: once there is room, the name is asked again.
    assert!(is_image(&providers.resolve("shield:3", 1.0).await));
}

#[tokio::test]
async fn absent_and_failed_answers_are_kept_and_unavailable_ones_asked_again() {
    for (answer, kept) in [
        (Answer::Absent, true),
        (Answer::Failed, true),
        (Answer::Malformed, true),
        (Answer::Unavailable, false),
    ] {
        let fake = Fake::new(answer.clone());
        let providers = registry(fake.clone(), ProviderLimits::default());
        let first = providers.resolve("shield:9", 1.0).await;
        let second = providers.resolve("shield:9", 1.0).await;
        if kept {
            assert!(matches!(first.as_deref(), Ok(Resolved::None)), "{answer:?}");
            assert!(
                matches!(second.as_deref(), Ok(Resolved::None)),
                "{answer:?}"
            );
            assert_eq!(fake.calls(), 1, "{answer:?} is remembered");
        } else {
            assert!(first.is_err() && second.is_err(), "{answer:?}");
            assert_eq!(fake.calls(), 2, "{answer:?} is asked again");
        }
    }
    let stats = |answer| {
        let fake = Fake::new(answer);
        let providers = registry(fake, ProviderLimits::default());
        providers.resolve("shield:9", 1.0).now_or_never();
        providers.stats()
    };
    assert_eq!(stats(Answer::Absent).absent, 1);
    assert_eq!(stats(Answer::Failed).failed, 1);
    assert_eq!(stats(Answer::Malformed).failed, 1);
    assert_eq!(stats(Answer::Unavailable).unavailable, 1);
    assert_eq!(stats(Answer::Image).images, 1);
}

#[tokio::test]
async fn a_new_generation_an_invalidation_or_another_pixel_ratio_asks_again() {
    let fake = Fake::new(Answer::Image);
    let providers = registry(fake.clone(), ProviderLimits::default());
    providers.resolve("shield:1", 1.0).await.expect("first");
    providers.resolve("shield:1", 2.0).await.expect("2x");
    assert_eq!(fake.calls(), 2, "each pixel ratio has its own image");
    *fake.generation.lock().expect("generation") = "pack-2".into();
    assert!(providers.known("shield:1", 1.0).is_none());
    providers.resolve("shield:1", 1.0).await.expect("new pack");
    assert_eq!(fake.calls(), 3, "a new pack makes the image again");
    assert!(providers.invalidate("shield"));
    assert!(!providers.invalidate("other"));
    assert!(providers.known("shield:1", 1.0).is_none());
    providers
        .resolve("shield:1", 1.0)
        .await
        .expect("invalidated");
    assert_eq!(fake.calls(), 4, "an invalidated namespace is asked again");
    assert!(providers.unregister("shield"));
    assert!(!providers.provides("shield:1"));
}

#[tokio::test]
async fn a_call_no_tile_waits_for_any_more_stops_and_frees_its_slot() {
    let (fake, _gate) = Fake::held();
    let providers = registry(fake.clone(), ProviderLimits::default());
    let mut ask = Box::pin(providers.resolve("shield:5", 1.0));
    assert!((&mut ask).now_or_never().is_none(), "the provider is held");
    assert_eq!(providers.stats().running, 1);
    drop(ask);
    let stats = providers.stats();
    assert_eq!((stats.cancelled, stats.running), (1, 0), "{stats:?}");
    assert!(providers.known("shield:5", 1.0).is_none());
}

#[tokio::test]
async fn answers_beyond_the_cache_budget_are_dropped_least_recent_first() {
    let fake = Fake::new(Answer::Image);
    let limits = ProviderLimits {
        cache_bytes: 600,
        ..ProviderLimits::default()
    };
    let providers = registry(fake.clone(), limits);
    // Each 8 x 8 image takes 256 bytes and some bookkeeping.
    for name in ["shield:a", "shield:b", "shield:c"] {
        providers.resolve(name, 4.0).await.expect("image");
    }
    assert!(providers.stats().cached_bytes <= 600);
    assert!(
        providers.known("shield:a", 4.0).is_none(),
        "the oldest is dropped"
    );
    assert!(providers.known("shield:c", 4.0).is_some());
}

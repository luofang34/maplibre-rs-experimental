//! Images made on request for the names a style asks for, such as road shields drawn from
//! a route's network and number, so that no host has to list every image in advance.
//!
//! A host registers a [`StyleImageProvider`] under a namespace. A name of the form
//! `namespace:id` that neither the sprite nor the style's own images supply is then asked of
//! that provider, from the tile worker that lays the label out, never from the render thread.
//! The label is drawn at once with whatever the style falls back to and laid out again when
//! the image arrives. The style itself only names images, so it still serializes; the
//! provider lives in the runtime, beside the worker's other services.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

use super::{AssetCache, AssetFailure};
use crate::style::StyleImage;

mod gate;
use gate::Gate;

/// What a provider is asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageRequest {
    /// The whole name the style asked for, namespace included.
    pub name: String,
    /// The part of the name after the namespace and its colon.
    pub id: String,
    /// Device pixels per layout pixel of the display the image is drawn on.
    pub pixel_ratio: f32,
}

/// A generated image and where it sits on its anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvidedImage {
    /// Straight-alpha RGBA pixels; `pixel_ratio` says how many of them cover a layout pixel.
    /// A colour image is never an SDF, so `sdf` stays false for it.
    pub image: StyleImage,
    /// The point, in image pixels from the top-left corner, that `icon-anchor` places as the
    /// image's centre; `None` uses the centre. A shield with a banner above it names the
    /// centre of the shield body here, so the body, not the whole picture, sits on the road.
    pub anchor: Option<[f32; 2]>,
}

/// A provider's answer for one name.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageResolution {
    /// The image to draw.
    Image(ProvidedImage),
    /// The name is understood but has no image, such as a route the rules draw no shield
    /// for. The style's fallback is drawn instead; the answer is kept like an image.
    Absent,
}

/// Why a provider has no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImageProviderError {
    /// A later request may succeed, such as when a resource is still downloading. The name
    /// is asked again when its tile is next requested.
    #[error("temporarily unavailable: {0}")]
    Unavailable(String),
    /// The name cannot be drawn, such as a malformed request. The failure is kept like an
    /// answer until the namespace is invalidated.
    #[error("cannot be provided: {0}")]
    Failed(String),
}

/// A provider's answer to come. Native workers run it on any thread, so with
/// `thread-safe-futures` it must be `Send`; a `Send` future fits either way.
#[cfg(feature = "thread-safe-futures")]
pub type ProvideFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ImageResolution, ImageProviderError>> + Send + 'a>>;
/// A provider's answer to come.
#[cfg(not(feature = "thread-safe-futures"))]
pub type ProvideFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ImageResolution, ImageProviderError>> + 'a>>;

/// Makes images for the names of one namespace.
///
/// Calls run on tile workers, concurrently, and may take as long as generation takes; the
/// registry runs each distinct request once at a time and bounds how many run together.
pub trait StyleImageProvider: Send + Sync + 'static {
    /// Identifies everything the images depend on besides the request, such as the hash of a
    /// rule pack. Answers made under another generation are not reused.
    fn generation(&self) -> String;

    /// Makes the image for `request`.
    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_>;
}

/// How much generation the registry allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderLimits {
    /// Generators running at once across every worker.
    pub max_running: usize,
    /// Requests waiting for a generator; more are refused as temporarily unavailable.
    pub max_waiting: usize,
    /// Bytes of answers kept, least recently used first out.
    pub cache_bytes: usize,
}

impl Default for ProviderLimits {
    fn default() -> Self {
        Self {
            max_running: 4,
            max_waiting: 1024,
            cache_bytes: 32 * 1024 * 1024,
        }
    }
}

/// Counts of what the registry did, for diagnostics and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageProviderStats {
    /// Lookups of a provided name by a tile.
    pub requested: u64,
    /// Lookups answered from the cache without waiting.
    pub cache_hits: u64,
    /// Provider calls made.
    pub calls: u64,
    /// Calls that returned an image.
    pub images: u64,
    /// Calls that returned [`ImageResolution::Absent`].
    pub absent: u64,
    /// Calls that failed for good.
    pub failed: u64,
    /// Calls that failed for now.
    pub unavailable: u64,
    /// Requests refused because the waiting queue was full.
    pub refused: u64,
    /// Calls dropped unfinished, because every tile waiting for them went away.
    pub cancelled: u64,
    /// Sources of tiles whose labels were laid out again because images arrived.
    pub relaid: u64,
    /// Generators running now.
    pub running: usize,
    /// Requests waiting for a generator now.
    pub waiting: usize,
    /// Bytes of answers held.
    pub cached_bytes: usize,
}

/// What the registry knows about a name.
#[derive(Debug)]
pub(crate) enum Resolved {
    /// An image to pack.
    Image(ProvidedImage),
    /// No image, now or until the namespace is invalidated.
    None,
}

struct Namespace {
    prefix: String,
    provider: Arc<dyn StyleImageProvider>,
    epoch: u64,
}

#[derive(Default)]
struct Counters {
    requested: AtomicU64,
    cache_hits: AtomicU64,
    calls: AtomicU64,
    images: AtomicU64,
    absent: AtomicU64,
    failed: AtomicU64,
    unavailable: AtomicU64,
    refused: AtomicU64,
    cancelled: AtomicU64,
    relaid: AtomicU64,
}

impl Counters {
    fn add(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

struct Registry {
    namespaces: Mutex<Vec<Namespace>>,
    epochs: AtomicU64,
    limits: Mutex<ProviderLimits>,
    cache: AssetCache,
    gate: Arc<Gate>,
    counters: Counters,
}

/// The providers a map's workers ask for images; clones share providers, answers and limits.
#[derive(Clone)]
pub struct ImageProviders(Arc<Registry>);

impl fmt::Debug for ImageProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let namespaces: Vec<String> = self
            .namespaces()
            .iter()
            .map(|namespace| namespace.prefix.clone())
            .collect();
        formatter
            .debug_struct("ImageProviders")
            .field("namespaces", &namespaces)
            .field("limits", &self.limits())
            .finish_non_exhaustive()
    }
}

impl Default for ImageProviders {
    fn default() -> Self {
        Self::with_limits(ProviderLimits::default())
    }
}

/// A provider call in progress; dropping it unfinished counts as a cancellation.
struct Call<'a> {
    counters: &'a Counters,
    finished: bool,
}

impl Drop for Call<'_> {
    fn drop(&mut self) {
        if !self.finished {
            Counters::add(&self.counters.cancelled);
        }
    }
}

impl ImageProviders {
    /// A registry without providers that generates within `limits`.
    pub fn with_limits(limits: ProviderLimits) -> Self {
        Self(Arc::new(Registry {
            namespaces: Mutex::default(),
            epochs: AtomicU64::new(0),
            limits: Mutex::new(limits),
            cache: AssetCache::with_budget(limits.cache_bytes),
            gate: Arc::default(),
            counters: Counters::default(),
        }))
    }

    fn namespaces(&self) -> MutexGuard<'_, Vec<Namespace>> {
        self.0
            .namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn limits(&self) -> ProviderLimits {
        *self.0.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes how many generators run and wait from now on; the cache keeps the budget it
    /// was made with.
    pub fn set_limits(&self, limits: ProviderLimits) {
        *self.0.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
    }

    /// Counts a source whose labels were laid out again with arrived images.
    pub(crate) fn count_relaid(&self) {
        Counters::add(&self.0.counters.relaid);
    }

    fn next_epoch(&self) -> u64 {
        self.0
            .epochs
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    /// Asks `provider` for the names that start with `namespace` and a colon, replacing any
    /// provider registered for it before.
    pub fn register(&self, namespace: &str, provider: Arc<dyn StyleImageProvider>) {
        let prefix = format!("{namespace}:");
        let epoch = self.next_epoch();
        let mut namespaces = self.namespaces();
        namespaces.retain(|existing| existing.prefix != prefix);
        namespaces.push(Namespace {
            prefix,
            provider,
            epoch,
        });
    }

    /// Stops asking for the names of `namespace`; returns whether a provider served it.
    pub fn unregister(&self, namespace: &str) -> bool {
        let prefix = format!("{namespace}:");
        let mut namespaces = self.namespaces();
        let before = namespaces.len();
        namespaces.retain(|existing| existing.prefix != prefix);
        namespaces.len() != before
    }

    /// Forgets every answer of `namespace`, so that the names are asked again; returns whether
    /// a provider serves it. A map re-lays the tiles that drew them (see
    /// `HeadlessMap::invalidate_provided_images`).
    pub fn invalidate(&self, namespace: &str) -> bool {
        let prefix = format!("{namespace}:");
        let epoch = self.next_epoch();
        let mut namespaces = self.namespaces();
        let Some(found) = namespaces
            .iter_mut()
            .find(|existing| existing.prefix == prefix)
        else {
            return false;
        };
        found.epoch = epoch;
        true
    }

    /// What the registry has done so far.
    pub fn stats(&self) -> ImageProviderStats {
        let counters = &self.0.counters;
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let (running, waiting) = self.0.gate.load();
        ImageProviderStats {
            requested: read(&counters.requested),
            cache_hits: read(&counters.cache_hits),
            calls: read(&counters.calls),
            images: read(&counters.images),
            absent: read(&counters.absent),
            failed: read(&counters.failed),
            unavailable: read(&counters.unavailable),
            refused: read(&counters.refused),
            cancelled: read(&counters.cancelled),
            relaid: read(&counters.relaid),
            running,
            waiting,
            cached_bytes: self.0.cache.approximate_bytes(),
        }
    }

    /// Whether a provider serves `name`.
    pub fn provides(&self, name: &str) -> bool {
        self.namespaces()
            .iter()
            .any(|namespace| name.starts_with(&namespace.prefix))
    }

    /// The provider for `name`, its request and the cache key of its answer.
    fn route(
        &self,
        name: &str,
        pixel_ratio: f32,
    ) -> Option<(Arc<dyn StyleImageProvider>, ImageRequest, String)> {
        let (provider, prefix, epoch) = self
            .namespaces()
            .iter()
            .find(|namespace| name.starts_with(&namespace.prefix))
            .map(|namespace| {
                (
                    namespace.provider.clone(),
                    namespace.prefix.len(),
                    namespace.epoch,
                )
            })?;
        let key = format!("{epoch}|{}|{pixel_ratio:.3}|{name}", provider.generation());
        let request = ImageRequest {
            name: name.to_owned(),
            id: name[prefix..].to_owned(),
            pixel_ratio,
        };
        Some((provider, request, key))
    }

    /// The answer for `name` if one is already known, without asking for it.
    pub(crate) fn known(&self, name: &str, pixel_ratio: f32) -> Option<Arc<Resolved>> {
        let (_, _, key) = self.route(name, pixel_ratio)?;
        Counters::add(&self.0.counters.requested);
        let known = self.0.cache.peek::<Resolved>(&key);
        if known.is_some() {
            Counters::add(&self.0.counters.cache_hits);
        }
        known
    }

    /// The answer for `name`, asking its provider once however many tiles wait for it.
    /// `Err` carries why it is unavailable for now.
    pub(crate) async fn resolve(
        &self,
        name: &str,
        pixel_ratio: f32,
    ) -> Result<Arc<Resolved>, String> {
        let Some((provider, request, key)) = self.route(name, pixel_ratio) else {
            return Ok(Arc::new(Resolved::None));
        };
        let registry = &self.0;
        let limits = self.limits();
        let answer = registry
            .cache
            .load(key, || async move {
                let Ok(_permit) =
                    Gate::enter(&registry.gate, limits.max_running, limits.max_waiting).await
                else {
                    Counters::add(&registry.counters.refused);
                    return Err(AssetFailure::Retryable(
                        "the image generation queue is full".to_owned(),
                    ));
                };
                Counters::add(&registry.counters.calls);
                let mut call = Call {
                    counters: &registry.counters,
                    finished: false,
                };
                let name = request.name.clone();
                let answer = provider.provide(request).await;
                call.finished = true;
                Self::keep(&registry.counters, &name, answer)
            })
            .await;
        answer.map_err(|failure| match failure {
            AssetFailure::Retryable(reason) | AssetFailure::Terminal(reason) => reason,
            AssetFailure::NotFound => "not found".to_owned(),
        })
    }

    /// What to keep of a provider's answer, and its size.
    fn keep(
        counters: &Counters,
        name: &str,
        answer: Result<ImageResolution, ImageProviderError>,
    ) -> Result<(Resolved, usize), AssetFailure> {
        // A remembered absence costs a key and a little bookkeeping.
        const ANSWER_BYTES: usize = 64;
        match answer {
            Ok(ImageResolution::Image(provided)) => {
                let image = &provided.image;
                let expected = image.width as usize * image.height as usize * 4;
                if image.width == 0
                    || image.height == 0
                    || image.data.len() != expected
                    || !(image.pixel_ratio.is_finite() && image.pixel_ratio > 0.0)
                {
                    Counters::add(&counters.failed);
                    tracing::warn!(%name, width = image.width, height = image.height, bytes = image.data.len(), "provided image is malformed");
                    return Ok((Resolved::None, ANSWER_BYTES));
                }
                Counters::add(&counters.images);
                let bytes = image.data.len() + name.len() + ANSWER_BYTES;
                Ok((Resolved::Image(provided), bytes))
            }
            Ok(ImageResolution::Absent) => {
                Counters::add(&counters.absent);
                Ok((Resolved::None, name.len() + ANSWER_BYTES))
            }
            Err(ImageProviderError::Failed(reason)) => {
                Counters::add(&counters.failed);
                tracing::warn!(%name, %reason, "image provider failed");
                Ok((Resolved::None, name.len() + ANSWER_BYTES))
            }
            Err(ImageProviderError::Unavailable(reason)) => {
                Counters::add(&counters.unavailable);
                tracing::warn!(%name, %reason, "provided image unavailable for now");
                Err(AssetFailure::Retryable(reason))
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

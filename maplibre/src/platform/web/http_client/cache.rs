//! Two-tier browser tile cache: bounded in-memory LRU cache plus persistent Cache Storage API.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use js_sys::{ArrayBuffer, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Cache, CacheStorage, Response, ResponseInit};

use crate::io::source_client::{ByteRange, CachePolicy};

/// Persistent browser Cache Storage bucket name for cached tiles and metadata.
pub const BROWSER_TILE_CACHE_NAME: &str = "maplibre-rs-tiles-v1";

/// Default 32 MiB cap for the shared in-memory browser LRU byte cache.
pub const BROWSER_MEMORY_CACHE_CAP_BYTES: usize = 32 * 1024 * 1024;
const BROWSER_MEMORY_CACHE_MAX_ITEM_BYTES: usize = 2 * 1024 * 1024;

struct LruState {
    entries: HashMap<Arc<str>, (Arc<[u8]>, u64)>,
    order: VecDeque<(Arc<str>, u64)>,
    bytes: usize,
    clock: u64,
    cap_bytes: usize,
}

impl LruState {
    fn new(cap_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            clock: 0,
            cap_bytes,
        }
    }

    fn get(&mut self, key_str: &str) -> Option<Arc<[u8]>> {
        let (key, (body, stamp)) = self.entries.get_key_value(key_str)?;
        let key = Arc::clone(key);
        let body = Arc::clone(body);
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        if *stamp != next_stamp {
            if let Some(entry) = self.entries.get_mut(key_str) {
                entry.1 = next_stamp;
            }
            self.order.push_back((key, next_stamp));
            self.compact_if_needed();
        }
        Some(body)
    }

    fn insert(&mut self, key_str: &str, body: &[u8]) {
        if body.len() > BROWSER_MEMORY_CACHE_MAX_ITEM_BYTES || body.len() > self.cap_bytes {
            return;
        }
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        let payload: Arc<[u8]> = Arc::from(body);
        if let Some((_, (old, _))) = self.entries.remove_entry(key_str) {
            self.bytes = self.bytes.saturating_sub(old.len());
        }
        while self.bytes.saturating_add(payload.len()) > self.cap_bytes {
            let Some((evict_key, evict_stamp)) = self.order.pop_front() else {
                self.entries.clear();
                self.bytes = 0;
                break;
            };
            if self
                .entries
                .get(evict_key.as_ref())
                .is_some_and(|(_, current)| *current == evict_stamp)
            {
                if let Some((evicted, _)) = self.entries.remove(evict_key.as_ref()) {
                    self.bytes = self.bytes.saturating_sub(evicted.len());
                }
            }
        }
        let key: Arc<str> = Arc::from(key_str);
        self.bytes = self.bytes.saturating_add(payload.len());
        self.entries.insert(Arc::clone(&key), (payload, next_stamp));
        self.order.push_back((key, next_stamp));
        self.compact_if_needed();
    }

    fn compact_if_needed(&mut self) {
        let limit = self.entries.len().saturating_mul(2).saturating_add(64);
        if self.order.len() <= limit {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(key, stamp)| {
            entries
                .get(key.as_ref())
                .is_some_and(|(_, current)| *current == *stamp)
        });
    }
}

/// Cloneable, bounded in-memory LRU byte cache shared across clones of [`super::WHATWGFetchHttpClient`].
#[derive(Clone)]
pub struct BrowserMemoryCache(Arc<Mutex<LruState>>);

impl Default for BrowserMemoryCache {
    fn default() -> Self {
        Self::new(BROWSER_MEMORY_CACHE_CAP_BYTES)
    }
}

impl BrowserMemoryCache {
    /// Creates a bounded in-memory LRU cache capped at `cap_bytes`.
    pub fn new(cap_bytes: usize) -> Self {
        Self(Arc::new(Mutex::new(LruState::new(cap_bytes))))
    }

    /// Looks up `key` and updates its LRU recency stamp on hit.
    pub fn get(&self, key: &str) -> Option<Arc<[u8]>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
    }

    /// Inserts `body` under `key`, evicting least-recently-used entries to stay under budget.
    pub fn insert(&self, key: &str, body: &[u8]) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, body);
    }

    /// Current byte footprint held in the in-memory cache.
    pub fn len_bytes(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).bytes
    }
}

/// Builds a deterministic memory-cache key incorporating optional byte range and request headers.
pub(super) fn memory_cache_key(
    url: &str,
    range: Option<ByteRange>,
    headers: &[(String, String)],
) -> String {
    let mut key = match range {
        Some(r) => format!("{url}#{}", r.header()),
        None => url.to_owned(),
    };
    for (name, value) in headers {
        key.push('|');
        key.push_str(&name.to_ascii_lowercase());
        key.push(':');
        key.push_str(value);
    }
    key
}

/// Whether a fetched response is eligible for persistent storage in the browser Cache Storage API.
pub(super) fn should_store_in_browser_cache(
    status: u16,
    range: Option<ByteRange>,
    policy: CachePolicy,
    cache_control: Option<&str>,
) -> bool {
    if status != 200 || range.is_some() || policy.is_volatile() {
        return false;
    }
    !has_disallowed_cache_control(cache_control)
}

fn has_disallowed_cache_control(cache_control: Option<&str>) -> bool {
    let Some(raw) = cache_control else {
        return false;
    };
    raw.split(',').any(|directive| {
        let token = directive
            .trim()
            .split('=')
            .next()
            .unwrap_or_default()
            .trim();
        token.eq_ignore_ascii_case("no-store") || token.eq_ignore_ascii_case("private")
    })
}

async fn open_tile_cache() -> Option<Cache> {
    let global = js_sys::global();
    let caches_val = Reflect::get(&global, &JsValue::from_str("caches")).ok()?;
    if caches_val.is_undefined() || caches_val.is_null() {
        return None;
    }
    let storage: CacheStorage = caches_val.dyn_into().ok()?;
    let opened = JsFuture::from(storage.open(BROWSER_TILE_CACHE_NAME))
        .await
        .ok()?;
    opened.dyn_into::<Cache>().ok()
}

/// Queries the persistent browser `CacheStorage` bucket (`"maplibre-rs-tiles-v1"`) for `url`.
/// Gracefully returns `None` when `caches` is unavailable (e.g. insecure origin or older runner).
pub(super) async fn match_browser_cache(url: &str) -> Option<Vec<u8>> {
    let cache = open_tile_cache().await?;
    let matched = JsFuture::from(cache.match_with_str(url)).await.ok()?;
    if matched.is_undefined() || matched.is_null() {
        return None;
    }
    let response: Response = matched.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let buffer_promise = response.array_buffer().ok()?;
    let buffer_val = JsFuture::from(buffer_promise).await.ok()?;
    let array_buffer: ArrayBuffer = buffer_val.dyn_into().ok()?;
    Some(Uint8Array::new(&array_buffer).to_vec())
}

/// Writes a full `200 OK` payload into `"maplibre-rs-tiles-v1"`, ignoring quota or origin errors.
pub(super) async fn put_browser_cache(url: &str, body: &[u8]) {
    let Some(cache) = open_tile_cache().await else {
        return;
    };
    let init = ResponseInit::new();
    init.set_status(200);
    let js_bytes = Uint8Array::from(body);
    let Ok(response) = Response::new_with_opt_buffer_source_and_init(Some(&js_bytes), &init) else {
        return;
    };
    let _ = JsFuture::from(cache.put_with_str(url, &response)).await;
}

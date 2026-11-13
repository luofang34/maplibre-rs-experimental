//! Bounded in-memory LRU tile cache and disk-cache concurrency gate for [`super::ReqwestHttpClient`].

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use tokio::sync::Semaphore;

pub(super) const MEMORY_CACHE_CAP_BYTES: usize = 64 * 1024 * 1024;
const MEMORY_CACHE_MAX_ITEM_BYTES: usize = 1024 * 1024;
const DISK_CACHE_IO_CONCURRENCY: usize = 6;

pub(super) struct MemoryTileCacheState {
    entries: HashMap<Arc<str>, (Arc<[u8]>, u64)>,
    order: VecDeque<(Arc<str>, u64)>,
    pub(super) bytes: usize,
    clock: u64,
    cap_bytes: usize,
}

impl MemoryTileCacheState {
    pub(super) fn new(cap_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            clock: 0,
            cap_bytes,
        }
    }

    pub(super) fn get(&mut self, url: &str) -> Option<Arc<[u8]>> {
        let (key, (body, stamp)) = self.entries.get_key_value(url)?;
        let key = Arc::clone(key);
        let body = Arc::clone(body);
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        if *stamp != next_stamp {
            if let Some(entry) = self.entries.get_mut(url) {
                entry.1 = next_stamp;
            }
            self.order.push_back((key, next_stamp));
            self.compact_order_if_needed();
        }
        Some(body)
    }

    pub(super) fn insert(&mut self, url: &str, body: &[u8]) {
        if body.len() > MEMORY_CACHE_MAX_ITEM_BYTES || body.len() > self.cap_bytes {
            return;
        }
        let next_stamp = self.clock.wrapping_add(1);
        self.clock = next_stamp;
        let payload: Arc<[u8]> = Arc::from(body);
        if let Some((_, (old_body, _))) = self.entries.remove_entry(url) {
            self.bytes = self.bytes.saturating_sub(old_body.len());
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
                .is_some_and(|(_, current_stamp)| *current_stamp == evict_stamp)
            {
                if let Some((evicted, _)) = self.entries.remove(evict_key.as_ref()) {
                    self.bytes = self.bytes.saturating_sub(evicted.len());
                }
            }
        }
        let key: Arc<str> = Arc::from(url);
        self.bytes = self.bytes.saturating_add(payload.len());
        self.entries.insert(Arc::clone(&key), (payload, next_stamp));
        self.order.push_back((key, next_stamp));
        self.compact_order_if_needed();
    }

    fn compact_order_if_needed(&mut self) {
        let limit = self.entries.len().saturating_mul(2).saturating_add(64);
        if self.order.len() <= limit {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(key, stamp)| {
            entries
                .get(key.as_ref())
                .is_some_and(|(_, current_stamp)| *current_stamp == *stamp)
        });
    }
}

pub(super) struct SharedCacheFrontend {
    pub(super) memory: Mutex<MemoryTileCacheState>,
    pub(super) io_gate: Semaphore,
}

impl SharedCacheFrontend {
    pub(super) fn new(cap_bytes: usize) -> Self {
        Self {
            memory: Mutex::new(MemoryTileCacheState::new(cap_bytes)),
            io_gate: Semaphore::new(DISK_CACHE_IO_CONCURRENCY),
        }
    }

    pub(super) fn get(&self, url: &str) -> Option<Arc<[u8]>> {
        self.memory.lock().ok()?.get(url)
    }

    pub(super) fn insert(&self, url: &str, body: &[u8]) {
        if let Ok(mut guard) = self.memory.lock() {
            guard.insert(url, body);
        }
    }
}

/// Shareable in-memory LRU cache and disk-I/O concurrency gate for [`super::ReqwestHttpClient`].
///
/// Cloning a [`super::ReqwestHttpClient`] already shares its own frontend; construct and pass an
/// [`HttpCacheFrontend`] explicitly when two independently built clients should share one.
#[derive(Clone)]
pub struct HttpCacheFrontend(pub(super) Arc<SharedCacheFrontend>);

impl Default for HttpCacheFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCacheFrontend {
    /// Creates a fresh in-memory cache frontend with the default 64 MiB budget.
    pub fn new() -> Self {
        Self(Arc::new(SharedCacheFrontend::new(MEMORY_CACHE_CAP_BYTES)))
    }
}

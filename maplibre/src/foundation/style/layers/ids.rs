//! Interned 32-bit identifiers for style layer and vector source-layer names.
//!
//! Replacing `String` layer keys in render-phase items, GPU resource tables, and queue filters
//! with `Copy` [`StyleLayerId`] / [`SourceLayerId`] handles eliminates per-frame string clones,
//! linear string comparisons, and `dlmalloc` allocator lock contention across workers.

use std::{
    cell::RefCell,
    fmt,
    sync::{OnceLock, RwLock},
};

use rustc_hash::FxHashMap;

struct GlobalInterner {
    strings: Vec<&'static str>,
    ids: FxHashMap<&'static str, u32>,
}

impl GlobalInterner {
    fn with_reserved(reserved: &[&'static str]) -> Self {
        let mut ids = FxHashMap::default();
        let mut strings = Vec::with_capacity(reserved.len().max(64));
        for (idx, &name) in reserved.iter().enumerate() {
            strings.push(name);
            ids.insert(name, idx as u32);
        }
        Self { strings, ids }
    }

    fn get_or_insert(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = u32::try_from(self.strings.len()).unwrap_or(u32::MAX);
        let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
        self.strings.push(leaked);
        self.ids.insert(leaked, id);
        id
    }
}

struct LocalInternerCache {
    strings: Vec<&'static str>,
    ids: FxHashMap<&'static str, u32>,
}

impl LocalInternerCache {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
            ids: FxHashMap::default(),
        }
    }

    fn sync_from(&mut self, global: &GlobalInterner) {
        let start = self.strings.len();
        if let Some(tail) = global.strings.get(start..) {
            self.strings.reserve(tail.len());
            for (offset, &leaked) in tail.iter().enumerate() {
                let id = (start + offset) as u32;
                self.strings.push(leaked);
                self.ids.insert(leaked, id);
            }
        }
    }
}

static STYLE_RESERVED: [&str; 3] = ["", "sky", "atmosphere"];
static SOURCE_RESERVED: [&str; 1] = [""];

static STYLE_GLOBAL: OnceLock<RwLock<GlobalInterner>> = OnceLock::new();
static SOURCE_GLOBAL: OnceLock<RwLock<GlobalInterner>> = OnceLock::new();

thread_local! {
    static STYLE_LOCAL: RefCell<LocalInternerCache> = RefCell::new(LocalInternerCache::new());
    static SOURCE_LOCAL: RefCell<LocalInternerCache> = RefCell::new(LocalInternerCache::new());
}

fn style_global() -> &'static RwLock<GlobalInterner> {
    STYLE_GLOBAL.get_or_init(|| RwLock::new(GlobalInterner::with_reserved(&STYLE_RESERVED)))
}

fn source_global() -> &'static RwLock<GlobalInterner> {
    SOURCE_GLOBAL.get_or_init(|| RwLock::new(GlobalInterner::with_reserved(&SOURCE_RESERVED)))
}

fn intern_with(
    name: &str,
    local: &'static std::thread::LocalKey<RefCell<LocalInternerCache>>,
    global_fn: fn() -> &'static RwLock<GlobalInterner>,
) -> u32 {
    local.with(|cell| {
        if let Some(&id) = cell.borrow().ids.get(name) {
            return id;
        }
        let lock = global_fn();
        {
            let guard = crate::util::sync::read_unpoisoned(lock);
            if let Some(&id) = guard.ids.get(name) {
                cell.borrow_mut().sync_from(&guard);
                return id;
            }
        }
        let mut guard = crate::util::sync::write_unpoisoned(lock);
        let id = guard.get_or_insert(name);
        cell.borrow_mut().sync_from(&guard);
        id
    })
}

fn resolve_with(
    id: u32,
    local: &'static std::thread::LocalKey<RefCell<LocalInternerCache>>,
    global_fn: fn() -> &'static RwLock<GlobalInterner>,
) -> &'static str {
    let idx = id as usize;
    local.with(|cell| {
        if let Some(&s) = cell.borrow().strings.get(idx) {
            return s;
        }
        let guard = crate::util::sync::read_unpoisoned(global_fn());
        let mut cache = cell.borrow_mut();
        cache.sync_from(&guard);
        cache.strings.get(idx).copied().unwrap_or("")
    })
}

/// Process-wide interned identifier of a [`StyleLayer::id`](crate::style::layer::StyleLayer::id).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StyleLayerId(pub u32);

impl StyleLayerId {
    /// Interned ID of the empty string `""`.
    pub const EMPTY: Self = Self(0);
    /// Pre-interned ID of the synthetic `"sky"` pass item.
    pub const SKY: Self = Self(1);
    /// Pre-interned ID of the synthetic `"atmosphere"` pass item.
    pub const ATMOSPHERE: Self = Self(2);

    /// Interns `name` in `O(1)` via thread-local cache after first observation on the thread.
    #[inline]
    pub fn intern(name: &str) -> Self {
        match name {
            "" => Self::EMPTY,
            "sky" => Self::SKY,
            "atmosphere" => Self::ATMOSPHERE,
            _ => Self(intern_with(name, &STYLE_LOCAL, style_global)),
        }
    }

    /// Resolves this ID back to its interned `&'static str` in `O(1)`.
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self.0 {
            0 => "",
            1 => "sky",
            2 => "atmosphere",
            id => resolve_with(id, &STYLE_LOCAL, style_global),
        }
    }
}

impl From<&str> for StyleLayerId {
    #[inline]
    fn from(value: &str) -> Self {
        Self::intern(value)
    }
}

impl From<&String> for StyleLayerId {
    #[inline]
    fn from(value: &String) -> Self {
        Self::intern(value.as_str())
    }
}

impl From<String> for StyleLayerId {
    #[inline]
    fn from(value: String) -> Self {
        Self::intern(value.as_str())
    }
}

impl AsRef<str> for StyleLayerId {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for StyleLayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Process-wide interned identifier of a vector [`StyleLayer::source_layer`](crate::style::layer::StyleLayer::source_layer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceLayerId(pub u32);

impl SourceLayerId {
    /// Interned ID of the empty string `""`.
    pub const EMPTY: Self = Self(0);

    /// Interns `name` in `O(1)` via thread-local cache after first observation on the thread.
    #[inline]
    pub fn intern(name: &str) -> Self {
        if name.is_empty() {
            Self::EMPTY
        } else {
            Self(intern_with(name, &SOURCE_LOCAL, source_global))
        }
    }

    /// Resolves this ID back to its interned `&'static str` in `O(1)`.
    #[inline]
    pub fn as_str(self) -> &'static str {
        if self.0 == 0 {
            ""
        } else {
            resolve_with(self.0, &SOURCE_LOCAL, source_global)
        }
    }
}

impl From<&str> for SourceLayerId {
    #[inline]
    fn from(value: &str) -> Self {
        Self::intern(value)
    }
}

impl From<&String> for SourceLayerId {
    #[inline]
    fn from(value: &String) -> Self {
        Self::intern(value.as_str())
    }
}

impl From<String> for SourceLayerId {
    #[inline]
    fn from(value: String) -> Self {
        Self::intern(value.as_str())
    }
}

impl AsRef<str> for SourceLayerId {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SourceLayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceLayerId, StyleLayerId};

    #[test]
    fn reserved_constants_and_roundtrip_match() {
        assert_eq!(StyleLayerId::intern(""), StyleLayerId::EMPTY);
        assert_eq!(StyleLayerId::intern("sky"), StyleLayerId::SKY);
        assert_eq!(StyleLayerId::intern("atmosphere"), StyleLayerId::ATMOSPHERE);
        assert_eq!(StyleLayerId::SKY.as_str(), "sky");
        assert_eq!(StyleLayerId::ATMOSPHERE.as_str(), "atmosphere");

        let road = StyleLayerId::intern("road_major_casing");
        assert_eq!(road, StyleLayerId::intern("road_major_casing"));
        assert_eq!(road.as_str(), "road_major_casing");

        let src = SourceLayerId::intern("transportation");
        assert_eq!(src, SourceLayerId::intern("transportation"));
        assert_eq!(src.as_str(), "transportation");
    }

    #[test]
    fn cross_thread_interning_produces_identical_ids() {
        let main_id = StyleLayerId::intern("building-3d-extrusion");
        let worker_id = std::thread::spawn(move || {
            assert_eq!(main_id.as_str(), "building-3d-extrusion");
            let w = StyleLayerId::intern("worker-created-layer");
            (StyleLayerId::intern("building-3d-extrusion"), w)
        })
        .join()
        .unwrap_or_default();
        assert_eq!(main_id, worker_id.0);
        assert_eq!(worker_id.1.as_str(), "worker-created-layer");
    }
}

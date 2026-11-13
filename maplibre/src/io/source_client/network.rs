//! Runtime network connectivity state and offline-mode error classification.

use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
};

use thiserror::Error;

/// Whether remote HTTP/HTTPS network sockets may be used to satisfy source misses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NetworkStatus {
    /// Remote HTTP and HTTPS network transports are permitted.
    #[default]
    Online,
    /// Remote HTTP and HTTPS misses fail immediately with [`OfflineError`] while local files,
    /// `mbtiles://`, `data:`, in-memory/disk caches, and offline stores remain active.
    Offline,
}

impl NetworkStatus {
    /// Returns `true` when [`Self::Online`].
    pub const fn is_online(self) -> bool {
        matches!(self, Self::Online)
    }

    /// Returns `true` when [`Self::Offline`].
    pub const fn is_offline(self) -> bool {
        matches!(self, Self::Offline)
    }
}

/// Alias for [`NetworkStatus`].
pub type Connectivity = NetworkStatus;

/// A remote HTTP/HTTPS resource miss was suppressed because [`NetworkState`] is [`NetworkStatus::Offline`].
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[error("network is offline; cannot fetch remote resource {url}")]
pub struct OfflineError {
    /// The remote URL whose network socket request was blocked while offline.
    pub url: String,
}

type StatusListener = Arc<dyn Fn(NetworkStatus) + Send + Sync>;

struct NetworkStateInner {
    online: AtomicBool,
    generation: AtomicU64,
    listeners: Mutex<Vec<StatusListener>>,
}

/// Cloneable, thread-safe connectivity handle shared by [`super::HttpSourceClient`] and transports.
#[derive(Clone)]
pub struct NetworkState(Arc<NetworkStateInner>);

impl Default for NetworkState {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NetworkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkState")
            .field("status", &self.status())
            .field("generation", &self.generation())
            .finish()
    }
}

impl NetworkState {
    /// Creates a connectivity handle initially set to [`NetworkStatus::Online`].
    pub fn new() -> Self {
        Self::with_status(NetworkStatus::Online)
    }

    /// Creates a connectivity handle initially set to [`NetworkStatus::Offline`].
    pub fn offline() -> Self {
        Self::with_status(NetworkStatus::Offline)
    }

    /// Creates a connectivity handle with the given initial `status`.
    pub fn with_status(status: NetworkStatus) -> Self {
        Self(Arc::new(NetworkStateInner {
            online: AtomicBool::new(status.is_online()),
            generation: AtomicU64::new(0),
            listeners: Mutex::new(Vec::new()),
        }))
    }

    /// Whether remote network transports are currently enabled.
    pub fn is_online(&self) -> bool {
        self.0.online.load(Ordering::Acquire)
    }

    /// Current [`NetworkStatus`] snapshot.
    pub fn status(&self) -> NetworkStatus {
        if self.is_online() {
            NetworkStatus::Online
        } else {
            NetworkStatus::Offline
        }
    }

    /// Monotonic counter incremented each time connectivity transitions between online and offline.
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// Sets whether remote network sockets are enabled, bumping [`Self::generation`] and notifying
    /// registered listeners when the state changes.
    pub fn set_online(&self, online: bool) {
        let prev = self.0.online.swap(online, Ordering::AcqRel);
        if prev != online {
            self.0.generation.fetch_add(1, Ordering::AcqRel);
            let status = if online {
                NetworkStatus::Online
            } else {
                NetworkStatus::Offline
            };
            let callbacks: Vec<StatusListener> = self
                .0
                .listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            for callback in callbacks {
                callback(status);
            }
        }
    }

    /// Updates connectivity to `status`.
    pub fn set_status(&self, status: NetworkStatus) {
        self.set_online(status.is_online());
    }

    /// Registers a listener invoked whenever connectivity changes between [`NetworkStatus::Online`]
    /// and [`NetworkStatus::Offline`].
    pub fn on_change(&self, listener: impl Fn(NetworkStatus) + Send + Sync + 'static) {
        self.0
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(listener));
    }
}

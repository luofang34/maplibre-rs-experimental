//! Thread-safe command queues and handles for driving a map across OS threads.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

use futures::channel::oneshot;

#[cfg(not(target_arch = "wasm32"))]
use super::MapClosed;
use super::{Map, Reply};
use crate::{
    environment::Environment,
    window::{HeadedMapWindow, MapWindowConfig},
};

/// Asks a map's owner thread to call [`SyncCommandQueue::apply`] or [`SyncMapCommands::apply`]
/// soon from any thread.
pub trait SyncWakeHost: Send + Sync {
    /// Requests that the owner applies pending commands.
    fn wake(&self);
}

impl<F: Fn() + Send + Sync> SyncWakeHost for F {
    fn wake(&self) {
        self()
    }
}

type SyncCommand<T> = Box<dyn FnOnce(&mut T) + Send>;
/// Returns `true` once it has answered, which removes it.
type SyncWatcher<T> = Box<dyn FnMut(&mut T) -> bool + Send>;

struct SyncShared<T> {
    commands: VecDeque<SyncCommand<T>>,
    watchers: Vec<SyncWatcher<T>>,
    closed: bool,
    /// A wake is on its way to the owner, which drains every command sent until then.
    wake_pending: bool,
}

fn lock<S>(mutex: &Mutex<S>) -> MutexGuard<'_, S> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The owner's side of a thread-safe command and watcher queue over any target `T`.
///
/// `T` itself need not be `Send` or `Sync`: it stays on the owner's thread while [`SyncHandle`]
/// clones send `Send` closures and await replies from any thread. Neither running commands nor
/// dropping closures ever holds the internal mutex.
pub struct SyncCommandQueue<T>(Arc<Mutex<SyncShared<T>>>);

impl<T: 'static> Default for SyncCommandQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> SyncCommandQueue<T> {
    /// Creates an open, empty command queue.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(SyncShared {
            commands: VecDeque::new(),
            watchers: Vec::new(),
            closed: false,
            wake_pending: false,
        })))
    }

    /// Creates a [`SyncHandle`] whose commands wake the owner through `wake`.
    pub fn handle(&self, wake: impl SyncWakeHost + 'static) -> SyncHandle<T> {
        SyncHandle {
            shared: Arc::clone(&self.0),
            wake: Arc::new(wake),
        }
    }

    /// Applies the commands sent so far in FIFO order, then evaluates active watchers.
    ///
    /// Each command runs with the mutex released, so commands may re-entrantly enqueue further
    /// commands or watchers without deadlocking.
    pub fn apply(&self, target: &mut T) {
        lock(&self.0).wake_pending = false;
        loop {
            let command = lock(&self.0).commands.pop_front();
            match command {
                Some(command) => command(target),
                None => break,
            }
        }
        self.observe(target);
    }

    /// Evaluates every active watcher against `target` after a frame, retaining unfinished ones.
    ///
    /// Watchers run and drop outside the mutex.
    pub fn observe(&self, target: &mut T) {
        let watchers = std::mem::take(&mut lock(&self.0).watchers);
        if watchers.is_empty() {
            return;
        }
        let mut kept = Vec::with_capacity(watchers.len());
        for mut watch in watchers {
            if !watch(target) {
                kept.push(watch);
            }
        }
        if kept.is_empty() {
            return;
        }
        let drop_outside = {
            let mut shared = lock(&self.0);
            if shared.closed {
                Some(kept)
            } else {
                // Watchers registered while these ran come after them.
                kept.append(&mut shared.watchers);
                shared.watchers = kept;
                None
            }
        };
        drop(drop_outside);
    }

    /// Refuses later commands and drops pending ones outside the mutex so every waiting [`Reply`]
    /// resolves to [`super::MapClosed`].
    pub fn close(&self) {
        let (commands, watchers) = {
            let mut shared = lock(&self.0);
            shared.closed = true;
            (
                std::mem::take(&mut shared.commands),
                std::mem::take(&mut shared.watchers),
            )
        };
        drop(commands);
        drop(watchers);
    }

    /// Whether [`Self::close`] has been called.
    pub fn is_closed(&self) -> bool {
        lock(&self.0).closed
    }

    /// Whether commands wait to be drained by [`Self::apply`].
    pub fn has_pending_commands(&self) -> bool {
        !lock(&self.0).commands.is_empty()
    }

    /// Whether [`SyncHandle::until`] watchers are currently waiting on `target`.
    pub fn has_watchers(&self) -> bool {
        !lock(&self.0).watchers.is_empty()
    }
}

impl<T> Drop for SyncCommandQueue<T> {
    fn drop(&mut self) {
        let (commands, watchers) = {
            let mut shared = lock(&self.0);
            shared.closed = true;
            (
                std::mem::take(&mut shared.commands),
                std::mem::take(&mut shared.watchers),
            )
        };
        drop(commands);
        drop(watchers);
    }
}

/// Thread-safe handle (`Clone + Send + Sync + 'static`) that sends commands to an owner's `T`.
pub struct SyncHandle<T> {
    shared: Arc<Mutex<SyncShared<T>>>,
    wake: Arc<dyn SyncWakeHost>,
}

impl<T> Clone for SyncHandle<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            wake: Arc::clone(&self.wake),
        }
    }
}

impl<T: 'static> SyncHandle<T> {
    fn push(&self, command: SyncCommand<T>) {
        let (wake, rejected) = {
            let mut shared = lock(&self.shared);
            if shared.closed {
                (false, Some(command))
            } else {
                shared.commands.push_back(command);
                (!std::mem::replace(&mut shared.wake_pending, true), None)
            }
        };
        drop(rejected);
        if wake {
            self.wake.wake();
        }
    }

    /// Enqueues `command` without allocating a reply channel. If the queue is already closed,
    /// `command` is dropped outside the mutex without waking the host.
    pub fn send_fire_and_forget(&self, command: impl FnOnce(&mut T) + Send + 'static) {
        self.push(Box::new(command));
    }

    /// Enqueues `command` and returns a [`Reply`] that resolves once the owner applies it.
    pub fn call<R: Send + 'static>(
        &self,
        command: impl FnOnce(&mut T) -> R + Send + 'static,
    ) -> Reply<R> {
        let (sender, receiver) = oneshot::channel();
        self.push(Box::new(move |target| {
            // The caller may have dropped its reply; the command still applies.
            sender.send(command(target)).ok();
        }));
        Reply(receiver)
    }

    /// Enqueues `command` and blocks the calling OS thread until the owner applies it or closes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn call_blocking<R: Send + 'static>(
        &self,
        command: impl FnOnce(&mut T) -> R + Send + 'static,
    ) -> Result<R, MapClosed> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.push(Box::new(move |target| {
            sender.send(command(target)).ok();
        }));
        receiver.recv().map_err(|_| MapClosed)
    }

    fn register_watcher(&self, watcher: SyncWatcher<T>) {
        let shared = Arc::downgrade(&self.shared);
        self.push(Box::new(move |_| {
            let rejected = if let Some(shared) = shared.upgrade() {
                let mut guard = lock(&shared);
                if guard.closed {
                    Some(watcher)
                } else {
                    guard.watchers.push(watcher);
                    None
                }
            } else {
                Some(watcher)
            };
            drop(rejected);
        }));
    }

    /// Replies with the first `Some` value `watch` returns, polling when the registration command
    /// is applied and after every subsequent frame or command batch.
    pub fn until<R: Send + 'static>(
        &self,
        mut watch: impl FnMut(&mut T) -> Option<R> + Send + 'static,
    ) -> Reply<R> {
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);
        let watcher: SyncWatcher<T> = Box::new(move |target| {
            let Some(reply) = sender.take() else {
                return true;
            };
            if reply.is_canceled() {
                return true;
            }
            match watch(target) {
                Some(value) => {
                    reply.send(value).ok();
                    true
                }
                None => {
                    sender = Some(reply);
                    false
                }
            }
        });
        self.register_watcher(watcher);
        Reply(receiver)
    }

    /// Blocks the calling OS thread until `watch` returns `Some` or the owner closes the queue.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn until_blocking<R: Send + 'static>(
        &self,
        mut watch: impl FnMut(&mut T) -> Option<R> + Send + 'static,
    ) -> Result<R, MapClosed> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let mut sender = Some(sender);
        let watcher: SyncWatcher<T> = Box::new(move |target| {
            let Some(reply) = sender.take() else {
                return true;
            };
            match watch(target) {
                Some(value) => {
                    reply.send(value).ok();
                    true
                }
                None => {
                    sender = Some(reply);
                    false
                }
            }
        });
        self.register_watcher(watcher);
        receiver.recv().map_err(|_| MapClosed)
    }

    /// Whether the owner has closed the queue, refusing subsequent commands.
    pub fn is_closed(&self) -> bool {
        lock(&self.shared).closed
    }
}

/// Thread-safe counterpart to [`super::MapCommands`] for a headed [`Map`].
pub struct SyncMapCommands<E: Environment> {
    queue: SyncCommandQueue<Map<E>>,
}

impl<E: Environment> Default for SyncMapCommands<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Environment> SyncMapCommands<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Creates an open command queue for a headed [`Map`].
    pub fn new() -> Self {
        Self {
            queue: SyncCommandQueue::new(),
        }
    }

    /// Creates a thread-safe [`SyncMapHandle`] whose commands wake the host through `wake`.
    pub fn handle(&self, wake: impl SyncWakeHost + 'static) -> SyncMapHandle<E> {
        self.queue.handle(wake)
    }

    /// Applies pending commands once the map's renderer is initialized; commands sent while the
    /// renderer is pending stay queued until a later call finds it ready.
    pub fn apply(&self, map: &mut Map<E>) {
        if map.is_initialized() {
            self.queue.apply(map);
        }
    }

    /// Evaluates active watchers after a frame when the map's renderer is initialized.
    pub fn observe(&self, map: &mut Map<E>) {
        if map.is_initialized() {
            self.queue.observe(map);
        }
    }

    /// Closes the queue and resolves pending replies to [`super::MapClosed`].
    pub fn close(&self) {
        self.queue.close();
    }

    /// Whether [`Self::close`] has been called.
    pub fn is_closed(&self) -> bool {
        self.queue.is_closed()
    }
}

/// Thread-safe handle (`Clone + Send + Sync + 'static`) to a headed [`Map`].
pub type SyncMapHandle<E> = SyncHandle<Map<E>>;

/// Thread-safe command queue for an offscreen [`crate::headless::map::HeadlessMap`].
#[cfg(feature = "headless")]
pub type SyncHeadlessCommands = SyncCommandQueue<crate::headless::map::HeadlessMap>;

/// Thread-safe handle (`Clone + Send + Sync + 'static`) to an offscreen
/// [`crate::headless::map::HeadlessMap`].
#[cfg(feature = "headless")]
pub type SyncHeadlessHandle = SyncHandle<crate::headless::map::HeadlessMap>;

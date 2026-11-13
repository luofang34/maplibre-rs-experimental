//! A host reaches a running map through commands that whoever owns the map applies between
//! frames, so nothing borrows the map while a frame or an input handler holds it, and the map
//! can be suspended, reset or rebuilt without invalidating the host's handles.
//!
//! The owner keeps [`MapCommands`] next to its [`Map`] and calls [`MapCommands::apply`] when
//! woken, [`MapCommands::observe`] after each frame and [`MapCommands::close`] when it stops.
//! Any host fits: a winit loop, a view driven by the platform's display link, or a headless
//! test.

use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use futures::channel::oneshot;

use super::Map;
use crate::{
    environment::Environment,
    window::{HeadedMapWindow, MapWindowConfig},
};

/// The map stopped before it could answer: its host shut it down or its lifecycle failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the map's host has stopped")]
pub struct MapClosed;

/// Asks the map's owner to call [`MapCommands::apply`] soon, from its own thread. It must
/// reach an owner that idles waiting for events, such as by sending one through
/// [`crate::event_loop::EventLoopProxy`].
pub trait WakeHost {
    /// Requests that the owner applies pending commands.
    fn wake(&self);
}

impl<F: Fn()> WakeHost for F {
    fn wake(&self) {
        self()
    }
}

type Command<T> = Box<dyn FnOnce(&mut T)>;
/// Returns `true` once it has answered, which removes it.
type Watcher<T> = Box<dyn FnMut(&mut T) -> bool>;

struct Shared<T> {
    commands: VecDeque<Command<T>>,
    watchers: Vec<Watcher<T>>,
    closed: bool,
    /// A wake is on its way to the owner, which drains every command sent until then.
    wake_pending: bool,
}

/// The owner's side of a queue over any target, which the map's queue specializes.
struct Queue<T>(Rc<RefCell<Shared<T>>>);

impl<T: 'static> Queue<T> {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(Shared {
            commands: VecDeque::new(),
            watchers: Vec::new(),
            closed: false,
            wake_pending: false,
        })))
    }

    fn sender(&self, wake: impl WakeHost + 'static) -> Sender<T> {
        Sender {
            shared: self.0.clone(),
            wake: Rc::new(wake),
        }
    }

    fn apply(&self, target: &mut T) {
        self.0.borrow_mut().wake_pending = false;
        loop {
            // Each command runs with the queue released, so it may send further commands.
            let command = self.0.borrow_mut().commands.pop_front();
            match command {
                Some(command) => command(target),
                None => break,
            }
        }
        self.observe(target);
    }

    fn observe(&self, target: &mut T) {
        let mut watchers = std::mem::take(&mut self.0.borrow_mut().watchers);
        if watchers.is_empty() {
            return;
        }
        watchers.retain_mut(|watch| !watch(target));
        let mut shared = self.0.borrow_mut();
        if shared.closed {
            drop(shared);
            drop(watchers);
            return;
        }
        // Watchers registered while these ran come after them.
        watchers.append(&mut shared.watchers);
        shared.watchers = watchers;
    }

    fn close(&self) {
        let (commands, watchers) = {
            let mut shared = self.0.borrow_mut();
            shared.closed = true;
            (
                std::mem::take(&mut shared.commands),
                std::mem::take(&mut shared.watchers),
            )
        };
        // Dropped with the queue released: a dropped reply sender wakes its receiver.
        drop(commands);
        drop(watchers);
    }
}

/// The host's side of a queue.
struct Sender<T> {
    shared: Rc<RefCell<Shared<T>>>,
    wake: Rc<dyn WakeHost>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl<T: 'static> Sender<T> {
    fn push(&self, command: Command<T>) {
        let wake = {
            let mut shared = self.shared.borrow_mut();
            if shared.closed {
                return;
            }
            shared.commands.push_back(command);
            !std::mem::replace(&mut shared.wake_pending, true)
        };
        if wake {
            self.wake.wake();
        }
    }

    fn call<R: 'static>(&self, command: impl FnOnce(&mut T) -> R + 'static) -> Reply<R> {
        let (sender, receiver) = oneshot::channel();
        self.push(Box::new(move |target| {
            // The host may have dropped its reply; the command still applies.
            sender.send(command(target)).ok();
        }));
        Reply(receiver)
    }

    fn until<R: 'static>(&self, mut watch: impl FnMut(&mut T) -> Option<R> + 'static) -> Reply<R> {
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);
        let watcher: Watcher<T> = Box::new(move |target| {
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
        let shared = Rc::downgrade(&self.shared);
        // Registered in order with the commands; the drain that registers it asks it first.
        self.push(Box::new(move |_| {
            if let Some(shared) = shared.upgrade() {
                shared.borrow_mut().watchers.push(watcher);
            }
        }));
        Reply(receiver)
    }

    fn is_closed(&self) -> bool {
        self.shared.borrow().closed
    }
}

/// What a command returns once the map's owner has applied it.
#[must_use = "a reply does nothing unless awaited; the command is applied either way"]
pub struct Reply<R>(pub(self) oneshot::Receiver<R>);

impl<R> Future for Reply<R> {
    type Output = Result<R, MapClosed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|reply| reply.map_err(|oneshot::Canceled| MapClosed))
    }
}

/// The commands [`MapHandle`]s send, kept by whoever owns the map.
pub struct MapCommands<E: Environment> {
    queue: Queue<Map<E>>,
}

impl<E: Environment> Default for MapCommands<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    fn default() -> Self {
        Self {
            queue: Queue::new(),
        }
    }
}

impl<E: Environment> MapCommands<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// A handle whose commands wake the owner through `wake`.
    pub fn handle(&self, wake: impl WakeHost + 'static) -> MapHandle<E> {
        MapHandle {
            sender: self.queue.sender(wake),
        }
    }

    /// Applies the commands sent so far in order, then asks every watcher again. A map whose
    /// renderer is not ready keeps them until a later call finds it ready.
    pub fn apply(&self, map: &mut Map<E>) {
        if map.is_initialized() {
            self.queue.apply(map);
        }
    }

    /// Asks every watcher about the map after a frame, keeping those still waiting.
    pub fn observe(&self, map: &mut Map<E>) {
        if map.is_initialized() {
            self.queue.observe(map);
        }
    }

    /// Refuses later commands and drops pending ones, so every reply still waiting resolves
    /// to [`MapClosed`].
    pub fn close(&self) {
        self.queue.close();
    }
}

/// Sends commands to a running map from its owner's thread.
///
/// The owner applies commands between frames, in the order they were sent, once the map's
/// renderer is ready; commands sent before then wait. A command is a closure over the map, so
/// everything [`Map`] offers is available without a mirror of its API. A handle stays valid
/// across suspension and window re-creation, and its replies resolve to [`MapClosed`] once
/// the owner closes the queue.
pub struct MapHandle<E: Environment> {
    sender: Sender<Map<E>>,
}

impl<E: Environment> Clone for MapHandle<E> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<E: Environment> MapHandle<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Applies `command` to the map and replies with what it returns.
    pub fn call<R: 'static>(&self, command: impl FnOnce(&mut Map<E>) -> R + 'static) -> Reply<R> {
        self.sender.call(command)
    }

    /// Replies with the first value `watch` returns, asking it when the command is applied and
    /// again after every frame and every later batch of commands, as a camera move is followed
    /// to its end.
    pub fn until<R: 'static>(
        &self,
        watch: impl FnMut(&mut Map<E>) -> Option<R> + 'static,
    ) -> Reply<R> {
        self.sender.until(watch)
    }

    /// Whether the owner has closed the queue, so commands are refused.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

mod sync_handle;
pub use sync_handle::{SyncCommandQueue, SyncHandle, SyncMapCommands, SyncMapHandle, SyncWakeHost};
#[cfg(feature = "headless")]
pub use sync_handle::{SyncHeadlessCommands, SyncHeadlessHandle};

#[cfg(all(
    feature = "headless",
    feature = "native-io",
    not(target_arch = "wasm32")
))]
mod headless_thread;
#[cfg(all(
    feature = "headless",
    feature = "native-io",
    not(target_arch = "wasm32")
))]
pub use headless_thread::{HeadlessRuntimeConfig, HeadlessRuntimeError, HeadlessRuntimeThread};

#[cfg(test)]
mod tests;

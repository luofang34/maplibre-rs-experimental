//! Typed map lifecycle, data, and missing-image event stream ([`MapEventBus`]).

#![deny(missing_docs)]

use std::{
    cell::RefCell,
    collections::VecDeque,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use futures::{
    channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
    Stream, StreamExt,
};
use rustc_hash::FxHashSet as HashSet;

use crate::{context::MapContext, render::frame_signals::ResourceReady};

/// Category of a runtime map error delivered via [`MapEvent::Error`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapErrorKind {
    /// Style parsing, validation, or runtime mutation failure.
    Style,
    /// Source metadata, tile fetch, or source data update failure.
    Source,
    /// Renderer, surface, or GPU pipeline failure.
    Renderer,
    /// Schedule or plugin execution failure.
    Schedule,
    /// Camera navigation or feature query failure.
    Operation,
}

/// Typed lifecycle, data, and error events emitted by a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapEvent {
    /// Emitted once when the initial style and visible frame first reach a settled state.
    MapLoaded,
    /// Emitted when a frame transitions from busy (pending tiles, animations, or redraws) to
    /// settled idle.
    MapIdle,
    /// Emitted when a style, source, renderer, schedule, or operation error occurs.
    Error {
        /// Category of the failure.
        kind: MapErrorKind,
        /// Human-readable description of the error.
        message: String,
    },
    /// Emitted when the initial style loads or when a runtime style mutation succeeds.
    StyleDataLoaded,
    /// Emitted when metadata or tile data for `source` becomes ready.
    SourceDataLoaded {
        /// Identifier of the style source whose data became ready.
        source: String,
    },
    /// Emitted once per missing icon or pattern identifier when a layer requests an image
    /// absent from the style's sprites, runtime images, and [`ImageProviders`].
    StyleImageMissing {
        /// Identifier of the requested missing style image.
        id: String,
    },
}

/// Opaque handle identifying a callback listener registered with [`MapEventBus::on_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

/// Pull-based async and non-blocking receiver for [`MapEvent`] notifications.
#[derive(Debug)]
pub struct MapEventReceiver {
    receiver: UnboundedReceiver<MapEvent>,
}

impl MapEventReceiver {
    /// Attempts to dequeue the next available [`MapEvent`] without blocking.
    pub fn try_recv(&mut self) -> Option<MapEvent> {
        self.receiver.try_recv().ok()
    }

    /// Drains all currently queued [`MapEvent`] items in FIFO order.
    pub fn drain(&mut self) -> Vec<MapEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.try_recv() {
            out.push(event);
        }
        out
    }

    /// Awaits the next [`MapEvent`], returning `None` only if all [`MapEventBus`] handles drop.
    pub async fn next(&mut self) -> Option<MapEvent> {
        StreamExt::next(&mut self.receiver).await
    }
}

impl Stream for MapEventReceiver {
    type Item = MapEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

type SharedCallback = Rc<RefCell<dyn FnMut(&MapEvent)>>;

#[derive(Default)]
struct MapEventBusInner {
    senders: Vec<UnboundedSender<MapEvent>>,
    listeners: Vec<(ListenerId, SharedCallback)>,
    next_listener_id: u64,
    initial_style_emitted: bool,
    map_loaded_emitted: bool,
    was_idle: bool,
    emitted_missing_images: HashSet<String>,
    buffered_ready_resources: VecDeque<ResourceReady>,
}

/// Shared event bus stored in `World::resources`, supporting both pull-based channel
/// subscriptions ([`Self::subscribe`]) and callback listeners ([`Self::on_event`]).
#[derive(Clone, Default)]
pub struct MapEventBus {
    inner: Rc<RefCell<MapEventBusInner>>,
}

impl std::fmt::Debug for MapEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.borrow();
        f.debug_struct("MapEventBus")
            .field("subscribers", &inner.senders.len())
            .field("listeners", &inner.listeners.len())
            .field("map_loaded_emitted", &inner.map_loaded_emitted)
            .field("was_idle", &inner.was_idle)
            .finish()
    }
}

impl MapEventBus {
    /// Creates a fresh empty event bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribes a new pull-based [`MapEventReceiver`] that receives all subsequent events.
    pub fn subscribe(&self) -> MapEventReceiver {
        let (tx, rx) = unbounded();
        self.inner.borrow_mut().senders.push(tx);
        MapEventReceiver { receiver: rx }
    }

    /// Registers a callback invoked synchronously whenever a [`MapEvent`] is emitted.
    pub fn on_event(&self, callback: impl FnMut(&MapEvent) + 'static) -> ListenerId {
        let mut inner = self.inner.borrow_mut();
        inner.next_listener_id = inner.next_listener_id.wrapping_add(1);
        let id = ListenerId(inner.next_listener_id);
        inner.listeners.push((id, Rc::new(RefCell::new(callback))));
        id
    }

    /// Removes an existing callback listener; returns whether it was present.
    pub fn remove_listener(&self, id: ListenerId) -> bool {
        let mut inner = self.inner.borrow_mut();
        let before = inner.listeners.len();
        inner.listeners.retain(|(existing, _)| *existing != id);
        inner.listeners.len() != before
    }

    /// Emits `event` to all active subscribers and callback listeners.
    pub fn emit(&self, event: MapEvent) {
        let callbacks: Vec<SharedCallback> = {
            let mut inner = self.inner.borrow_mut();
            inner
                .senders
                .retain(|sender| sender.unbounded_send(event.clone()).is_ok());
            inner.listeners.iter().map(|(_, cb)| cb.clone()).collect()
        };
        for cb in callbacks {
            if let Ok(mut f) = cb.try_borrow_mut() {
                f(&event);
            }
        }
    }

    /// Emits [`MapEvent::Error`] with `kind` and `message`.
    pub fn emit_error(&self, kind: MapErrorKind, message: impl Into<String>) {
        self.emit(MapEvent::Error {
            kind,
            message: message.into(),
        });
    }

    /// Emits [`MapEvent::StyleImageMissing`] for `id` if it has not yet been reported in the
    /// current style session.
    pub fn emit_missing_image(&self, id: impl Into<String>) -> bool {
        let id = id.into();
        if id.is_empty() {
            return false;
        }
        let inserted = self
            .inner
            .borrow_mut()
            .emitted_missing_images
            .insert(id.clone());
        if inserted {
            self.emit(MapEvent::StyleImageMissing { id });
        }
        inserted
    }

    /// Clears `id` from the reported missing-image set so a future request emits
    /// [`MapEvent::StyleImageMissing`] again.
    pub fn clear_missing_image(&self, id: &str) {
        self.inner.borrow_mut().emitted_missing_images.remove(id);
    }

    /// Resets lifecycle tracking when a map or style is reset, allowing missing images and idle
    /// transitions to be emitted afresh.
    pub fn reset_style_state(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.initial_style_emitted = false;
        inner.map_loaded_emitted = false;
        inner.was_idle = false;
        inner.emitted_missing_images.clear();
    }

    /// Marks the map as busy so the next settled frame emits [`MapEvent::MapIdle`].
    pub fn mark_busy(&self) {
        self.inner.borrow_mut().was_idle = false;
    }

    pub(crate) fn mark_initial_style_emitted(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        let already = inner.initial_style_emitted;
        inner.initial_style_emitted = true;
        already
    }

    pub(crate) fn observe_frame_settled(&self, settled: bool) {
        if !settled {
            self.inner.borrow_mut().was_idle = false;
            return;
        }
        let (emit_loaded, emit_idle) = {
            let mut inner = self.inner.borrow_mut();
            let loaded = !inner.map_loaded_emitted;
            let idle = !inner.was_idle;
            inner.map_loaded_emitted = true;
            inner.was_idle = true;
            (loaded, idle)
        };
        if emit_loaded {
            self.emit(MapEvent::MapLoaded);
        }
        if emit_idle {
            self.emit(MapEvent::MapIdle);
        }
    }

    pub(crate) fn push_ready_resources(&self, items: impl IntoIterator<Item = ResourceReady>) {
        const MAX_BUFFERED_READY: usize = 1024;
        let mut inner = self.inner.borrow_mut();
        inner.buffered_ready_resources.extend(items);
        while inner.buffered_ready_resources.len() > MAX_BUFFERED_READY {
            inner.buffered_ready_resources.pop_front();
        }
    }

    pub(crate) fn take_buffered_ready_resources(&self) -> Vec<ResourceReady> {
        self.inner
            .borrow_mut()
            .buffered_ready_resources
            .drain(..)
            .collect()
    }
}

impl MapContext {
    /// Returns the shared [`MapEventBus`] stored in this context's world resources.
    pub fn event_bus(&mut self) -> MapEventBus {
        self.world
            .resources
            .get_or_init_mut::<MapEventBus>()
            .clone()
    }

    /// Subscribes a pull-based [`MapEventReceiver`] to this map context's [`MapEventBus`].
    pub fn subscribe_events(&mut self) -> MapEventReceiver {
        self.event_bus().subscribe()
    }

    /// Registers a callback listener on this map context's [`MapEventBus`].
    pub fn on_event(&mut self, listener: impl FnMut(&MapEvent) + 'static) -> ListenerId {
        self.event_bus().on_event(listener)
    }
}

#[cfg(test)]
mod tests;

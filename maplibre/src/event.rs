//! Typed map lifecycle, data, and missing-image event stream ([`MapEventBus`]).

#![deny(missing_docs)]

use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use futures::{
    channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
    Stream, StreamExt,
};

use crate::{
    context::MapContext,
    io::tile_retry::RequestKind,
    render::frame_signals::ResourceReady,
    sdf::assets::ImageProviders,
    style::{
        layer::{ImageName, LayerPaint, PatternName, StyleProperty},
        mutation::{StyleChange, StyleMutationError},
        source::Source,
        Style, StyleImage,
    },
};

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
    emitted_missing_images: BTreeSet<String>,
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

    /// Removes a previously registered callback listener; returns whether it was present.
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
        self.inner
            .borrow_mut()
            .buffered_ready_resources
            .extend(items);
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

    /// Adds or replaces a runtime style image (`id`) and refreshes symbol and pattern layers so
    /// loaded tiles can pack it into their atlases.
    pub fn add_style_image(
        &mut self,
        id: &str,
        image: StyleImage,
    ) -> Result<StyleChange, StyleMutationError> {
        let bus = self.event_bus();
        match self.mutate_style(|style| {
            let mut change = style.add_image(id, image);
            for layer in &style.layers {
                if matches!(
                    layer.type_.as_str(),
                    "fill" | "fill-extrusion" | "line" | "background"
                ) && !change.layers.contains(&layer.id)
                {
                    change.layers.push(layer.id.clone());
                    if layer.type_ != "background" {
                        change.redraw_tiles = true;
                    }
                }
            }
            Ok(change)
        }) {
            Ok(change) => {
                bus.mark_busy();
                bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                bus.emit_error(MapErrorKind::Style, error.to_string());
                Err(error)
            }
        }
    }

    /// Removes a runtime style image (`id`) and clears its missing-image deduplication entry so
    /// subsequent requests for `id` emit [`MapEvent::StyleImageMissing`] again.
    pub fn remove_style_image(&mut self, id: &str) -> Result<StyleChange, StyleMutationError> {
        let bus = self.event_bus();
        bus.clear_missing_image(id);
        match self.mutate_style(|style| Ok(style.remove_image(id))) {
            Ok(change) => {
                bus.mark_busy();
                bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                bus.emit_error(MapErrorKind::Style, error.to_string());
                Err(error)
            }
        }
    }

    /// Marks [`MapEventBus`] busy if this context currently requires a redraw or `extra_busy` holds.
    pub(crate) fn mark_busy_if_needed(&mut self, extra_busy: bool) {
        if self.needs_redraw() || extra_busy {
            self.event_bus().mark_busy();
        }
    }

    /// Synchronizes frame-end lifecycle, ready-source, and missing-image events into [`MapEventBus`].
    pub(crate) fn sync_frame_events(
        &mut self,
        image_providers: Option<&ImageProviders>,
        extra_busy: bool,
    ) {
        let bus = self.event_bus();
        if !bus.mark_initial_style_emitted() {
            bus.emit(MapEvent::StyleDataLoaded);
        }

        let ready = self.take_ready_resources();
        if !ready.is_empty() {
            let mut loaded_sources = BTreeSet::new();
            for item in &ready {
                if let ResourceReady::Tile {
                    kind, loaded: true, ..
                } = item
                {
                    for (name, source) in &self.style.sources {
                        let matches_kind = match kind {
                            RequestKind::Vector => {
                                matches!(source, Source::Vector(_) | Source::GeoJson(_))
                            }
                            RequestKind::Raster => matches!(
                                source,
                                Source::Raster(_)
                                    | Source::Image(_)
                                    | Source::Video(_)
                                    | Source::Canvas(_)
                            ),
                            RequestKind::Dem => matches!(source, Source::RasterDem(_)),
                        };
                        if matches_kind {
                            loaded_sources.insert(name.clone());
                        }
                    }
                }
            }
            bus.push_ready_resources(ready);
            for source in loaded_sources {
                bus.emit(MapEvent::SourceDataLoaded { source });
            }
        }

        let mut missing_ids = BTreeSet::new();
        if let Some(providers) = image_providers {
            for id in providers.take_missing() {
                missing_ids.insert(id);
            }
        }
        collect_visible_missing_style_images(
            &self.style,
            self.view_state.zoom().value(),
            image_providers,
            &mut missing_ids,
        );
        for id in missing_ids {
            if !self.style.images.contains_key(&id) {
                bus.emit_missing_image(id);
            }
        }

        let settled = !self.needs_redraw() && !extra_busy;
        bus.observe_frame_settled(settled);
    }
}

fn collect_visible_missing_style_images(
    style: &Style,
    zoom: f64,
    providers: Option<&ImageProviders>,
    out: &mut BTreeSet<String>,
) {
    let is_supplied = |name: &str| -> bool {
        style.images.contains_key(name) || providers.is_some_and(|p| p.provides(name))
    };
    for layer in &style.layers {
        if !layer.is_visible_at(zoom) {
            continue;
        }
        let Some(paint) = &layer.paint else {
            continue;
        };
        let pattern_val = match paint {
            LayerPaint::Fill(p) => p.fill_pattern.as_ref(),
            LayerPaint::FillExtrusion(p) => p.fill_extrusion_pattern.as_ref(),
            LayerPaint::Background(p) => p.background_pattern.as_ref(),
            LayerPaint::Line(p) => p.line_pattern.as_ref(),
            _ => None,
        };
        if let Some(val) = pattern_val {
            let prop = StyleProperty::<PatternName>::parse(val);
            if prop.is_feature_constant()
                && let Some(PatternName(name)) = prop.evaluate_at_zoom(zoom)
                && !name.is_empty()
                && style.sprite.is_none()
                && !is_supplied(&name)
            {
                out.insert(name);
            }
        }
        if style.sprite.is_none()
            && let LayerPaint::Symbol(sym) = paint
            && let Some(val) = sym.properties.get("icon-image")
        {
            let prop = StyleProperty::<ImageName>::parse(val);
            if prop.is_feature_constant()
                && let Some(ImageName(name)) = prop.evaluate_at_zoom(zoom)
                && !name.is_empty()
                && !is_supplied(&name)
            {
                out.insert(name);
            }
        }
    }
}

#[cfg(test)]
mod tests;

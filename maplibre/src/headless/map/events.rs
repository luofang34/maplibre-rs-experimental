//! Typed map event subscription and runtime style-image registration on [`HeadlessMap`].

use super::HeadlessMap;
use crate::{
    event::{ListenerId, MapEvent, MapEventBus, MapEventReceiver},
    style::{
        mutation::{StyleChange, StyleMutationError},
        StyleImage,
    },
};

impl HeadlessMap {
    /// Returns the shared [`MapEventBus`] stored in this headless map's world resources.
    pub fn event_bus(&mut self) -> MapEventBus {
        self.map_context.event_bus()
    }

    /// Subscribes a pull-based [`MapEventReceiver`] receiving all subsequent [`MapEvent`]s.
    pub fn subscribe_events(&mut self) -> MapEventReceiver {
        self.map_context.subscribe_events()
    }

    /// Registers a callback listener invoked synchronously when any [`MapEvent`] is emitted.
    pub fn on_event(&mut self, listener: impl FnMut(&MapEvent) + 'static) -> ListenerId {
        self.map_context.on_event(listener)
    }

    /// Removes a callback listener previously registered via [`Self::on_event`].
    pub fn remove_event_listener(&mut self, id: ListenerId) -> bool {
        self.map_context.event_bus().remove_listener(id)
    }

    /// Adds or replaces a runtime style image (`id`) and refreshes symbol and pattern layers.
    pub fn add_style_image(
        &mut self,
        id: &str,
        image: StyleImage,
    ) -> Result<StyleChange, StyleMutationError> {
        self.map_context.add_style_image(id, image)
    }

    /// Removes a runtime style image (`id`) and clears its missing-image deduplication state.
    pub fn remove_style_image(&mut self, id: &str) -> Result<StyleChange, StyleMutationError> {
        self.map_context.remove_style_image(id)
    }
}

//! Typed map event subscription and runtime style-image registration on [`Map`].

use super::{CurrentMapContext, Map, MapError};
use crate::{
    environment::Environment,
    event::{ListenerId, MapEvent, MapEventBus, MapEventReceiver},
    style::{mutation::StyleChange, StyleImage},
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Returns the map's [`MapEventBus`], available both before and after renderer initialization.
    pub fn event_bus(&self) -> &MapEventBus {
        &self.event_bus
    }

    /// Subscribes a pull-based [`MapEventReceiver`] receiving all subsequent [`MapEvent`]s.
    pub fn subscribe_events(&self) -> MapEventReceiver {
        self.event_bus.subscribe()
    }

    /// Registers a callback listener invoked synchronously when any [`MapEvent`] is emitted.
    pub fn on_event(&self, listener: impl FnMut(&MapEvent) + 'static) -> ListenerId {
        self.event_bus.on_event(listener)
    }

    /// Removes a callback listener previously registered via [`Self::on_event`].
    pub fn remove_event_listener(&self, id: ListenerId) -> bool {
        self.event_bus.remove_listener(id)
    }

    /// Adds or replaces a runtime style image (`id`), before or after renderer initialization.
    pub fn add_style_image(
        &mut self,
        id: &str,
        image: StyleImage,
    ) -> Result<StyleChange, MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.add_style_image(id, image)?),
            CurrentMapContext::Pending(pending) => {
                let change = pending.style.add_image(id, image);
                self.event_bus.mark_busy();
                self.event_bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
        }
    }

    /// Removes a runtime style image (`id`) and clears its missing-image deduplication state.
    pub fn remove_style_image(&mut self, id: &str) -> Result<StyleChange, MapError> {
        self.event_bus.clear_missing_image(id);
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.remove_style_image(id)?),
            CurrentMapContext::Pending(pending) => {
                let change = pending.style.remove_image(id);
                self.event_bus.mark_busy();
                self.event_bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
        }
    }
}

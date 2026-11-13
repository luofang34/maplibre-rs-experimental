//! Custom style-layer registration on [`Map`].

use super::{CurrentMapContext, Map};
use crate::{
    custom_layer::CustomStyleLayer,
    environment::Environment,
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Registers a [`CustomStyleLayer`] drawn at the position of the matching `"type": "custom"`
    /// layer in `style.layers`, before or after renderer initialization.
    pub fn register_custom_layer(&mut self, layer: impl CustomStyleLayer) {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.register_custom_layer(layer),
            CurrentMapContext::Pending(_) => self.custom_layers.register(layer),
        }
    }

    /// Removes a registered [`CustomStyleLayer`] by `id`, returning whether one existed.
    pub fn remove_custom_layer(&mut self, id: &str) -> bool {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.remove_custom_layer(id),
            CurrentMapContext::Pending(_) => self.custom_layers.remove(id),
        }
    }
}

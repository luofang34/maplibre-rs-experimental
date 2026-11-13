//! Custom style-layer registration on [`HeadlessMap`].

use super::HeadlessMap;
use crate::custom_layer::CustomStyleLayer;

impl HeadlessMap {
    /// Registers a [`CustomStyleLayer`] drawn at the position of the matching `"type": "custom"`
    /// layer in `style.layers`.
    pub fn register_custom_layer(&mut self, layer: impl CustomStyleLayer) {
        self.map_context.register_custom_layer(layer);
    }

    /// Removes a registered [`CustomStyleLayer`] by `id`, returning whether one existed.
    pub fn remove_custom_layer(&mut self, id: &str) -> bool {
        self.map_context.remove_custom_layer(id)
    }
}

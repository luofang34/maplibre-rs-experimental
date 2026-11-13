//! Adding, removing and updating a map's sources and runtime `feature-state`.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{CurrentMapContext, Map, MapError};
use crate::{
    environment::Environment,
    style::{
        feature_state::{FeatureId, FeatureStateTarget},
        mutation::StyleChange,
        source::Source,
    },
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Adds a source.
    pub fn add_source(&mut self, name: &str, source: Source) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.add_source(name, source))
    }

    /// Removes a source no layer draws from.
    pub fn remove_source(&mut self, name: &str) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.remove_source(name))
    }

    /// Points an image source at a picture and optionally other corners, as GL JS
    /// `ImageSource.updateImage`; see [`crate::style::Style::update_image_source`].
    pub fn update_image_source(
        &mut self,
        name: &str,
        url: String,
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.update_image_source(name, url, coordinates))
    }

    /// Stretches an image source over other corners, as GL JS `ImageSource.setCoordinates`;
    /// see [`crate::style::Style::set_image_coordinates`].
    pub fn set_image_coordinates(
        &mut self,
        name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_image_coordinates(name, coordinates))
    }

    /// Merges `state` into the runtime state of feature `id` in `target`, before or after the
    /// renderer is initialized, as GL JS `setFeatureState`.
    pub fn set_feature_state(
        &mut self,
        target: FeatureStateTarget,
        id: impl Into<FeatureId>,
        state: serde_json::Map<String, Value>,
    ) {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.set_feature_state(target, id, state),
            CurrentMapContext::Pending(pending) => {
                pending.style.set_feature_state(target, id, state);
            }
        }
    }

    /// Returns the runtime state map of feature `id` in `target`, if any, as GL JS
    /// `getFeatureState`.
    pub fn feature_state(
        &self,
        target: &FeatureStateTarget,
        id: impl Into<FeatureId>,
    ) -> Option<&BTreeMap<String, Value>> {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.feature_state(target, id),
            CurrentMapContext::Pending(pending) => pending.style.feature_state(target, id),
        }
    }

    /// Removes `key` (or all keys when `None`) from feature `id` (or all features of `target` when
    /// `id` is `None`), before or after the renderer is initialized, as GL JS `removeFeatureState`.
    pub fn remove_feature_state(
        &mut self,
        target: &FeatureStateTarget,
        id: Option<FeatureId>,
        key: Option<&str>,
    ) {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.remove_feature_state(target, id, key),
            CurrentMapContext::Pending(pending) => {
                pending.style.remove_feature_state(target, id, key);
            }
        }
    }
}

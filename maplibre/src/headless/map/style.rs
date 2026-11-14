//! Changing a headless map's style and runtime `feature-state` between frames, as [`crate::map::Map`] changes its own.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::HeadlessMap;
use crate::style::{
    feature_state::{FeatureId, FeatureStateTarget},
    mutation::{StyleChange, StyleMutationError},
    Style,
};

impl HeadlessMap {
    /// Replaces a GeoJSON source's data and refreshes the loaded tiles it changes, as
    /// [`crate::context::MapContext::set_geojson_data`] does.
    #[cfg(feature = "geojson")]
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: crate::style::source::GeoJsonData,
    ) -> Result<(), crate::geojson::update::SourceUpdateError> {
        let bus = self.map_context.event_bus();
        match self.map_context.set_geojson_data(source_name, data) {
            Ok(()) => {
                bus.mark_busy();
                bus.emit(crate::event::MapEvent::SourceDataLoaded {
                    source: source_name.to_owned(),
                });
                Ok(())
            }
            Err(error) => {
                bus.emit_error(crate::event::MapErrorKind::Source, error.to_string());
                Err(error)
            }
        }
    }

    /// Replaces a GeoJSON source's data with the inline document `geojson`.
    #[cfg(feature = "geojson")]
    pub fn set_geojson_source(
        &mut self,
        source_name: &str,
        geojson: serde_json::Value,
    ) -> Result<(), crate::geojson::update::SourceUpdateError> {
        self.set_geojson_data(
            source_name,
            crate::style::source::GeoJsonData::Inline(std::sync::Arc::new(geojson)),
        )
    }

    /// Replaces the entire style via [`Style::diff_and_replace`], retaining loaded tiles for
    /// unchanged sources and refreshing affected GPU state on the next frame.
    pub fn set_style(&mut self, next: Style) -> crate::style::diff::StyleReplacementDiff {
        let bus = self.map_context.event_bus();
        let diff = self.map_context.set_style(next);
        bus.mark_busy();
        bus.emit(crate::event::MapEvent::StyleDataLoaded);
        diff
    }

    /// Pushes a straight-alpha RGBA8 frame (`width * height * 4` bytes) and optional updated
    /// `[longitude, latitude]` corner coordinates onto a `"video"` source.
    pub fn set_video_source_frame(
        &mut self,
        source_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.mutate_style(|style| {
            style.set_video_source_frame(source_name, width, height, rgba, coordinates)
        })
    }

    /// Updates the four `[longitude, latitude]` corners of a `"video"` source.
    pub fn set_video_coordinates(
        &mut self,
        source_name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        self.mutate_style(|style| style.set_video_coordinates(source_name, coordinates))
    }

    /// Pushes a straight-alpha RGBA8 frame (`width * height * 4` bytes) and optional updated
    /// `[longitude, latitude]` corner coordinates onto a `"canvas"` source.
    pub fn set_canvas_source_frame(
        &mut self,
        source_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.mutate_style(|style| {
            style.set_canvas_source_frame(source_name, width, height, rgba, coordinates)
        })
    }

    /// Updates the four `[longitude, latitude]` corners of a `"canvas"` source.
    pub fn set_canvas_coordinates(
        &mut self,
        source_name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        self.mutate_style(|style| style.set_canvas_coordinates(source_name, coordinates))
    }

    /// Applies a style change and drops or reloads what it leaves stale, as
    /// [`crate::context::MapContext::mutate_style`] does.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        let bus = self.map_context.event_bus();
        match self.map_context.mutate_style(apply) {
            Ok(change) => {
                bus.mark_busy();
                bus.emit(crate::event::MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                bus.emit_error(crate::event::MapErrorKind::Style, error.to_string());
                Err(error)
            }
        }
    }

    /// Merges `state` into the runtime state of feature `id` in `target`, updating resident GPU
    /// buffers and rendered-feature queries in-place on the next frame.
    pub fn set_feature_state(
        &mut self,
        target: FeatureStateTarget,
        id: impl Into<FeatureId>,
        state: Map<String, Value>,
    ) {
        self.map_context.set_feature_state(target, id, state);
    }

    /// Returns the runtime state map of feature `id` in `target`, if any.
    pub fn feature_state(
        &self,
        target: &FeatureStateTarget,
        id: impl Into<FeatureId>,
    ) -> Option<&BTreeMap<String, Value>> {
        self.map_context.feature_state(target, id)
    }

    /// Removes `key` (or all keys when `None`) from feature `id` (or all features of `target` when
    /// `id` is `None`), updating resident GPU buffers and rendered-feature queries in-place.
    pub fn remove_feature_state(
        &mut self,
        target: &FeatureStateTarget,
        id: Option<FeatureId>,
        key: Option<&str>,
    ) {
        self.map_context.remove_feature_state(target, id, key);
    }

    /// Sets a `global-state` value, as GL JS `setGlobalStateProperty`; `null` restores the
    /// declared default, refreshing paint-only layers in-place on the next frame.
    pub fn set_global_state(&mut self, key: &str, value: Value) {
        self.map_context.set_global_state(key, value);
    }

    /// Every declared or set `global-state` key with its current value.
    pub fn global_state(&self) -> BTreeMap<String, Value> {
        self.map_context.style.global_state_values()
    }
}

#[cfg(feature = "experimental")]
mod sky;
#[cfg(test)]
mod tests;

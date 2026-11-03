//! Changing a headless map's style between frames, as [`crate::map::Map`] changes its own.

use super::HeadlessMap;
use crate::style::{
    mutation::{StyleChange, StyleMutationError},
    Style,
};

impl HeadlessMap {
    /// Replaces a GeoJSON source's data and refreshes the loaded tiles it changes, as
    /// [`crate::context::MapContext::set_geojson_data`] does.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: crate::style::source::GeoJsonData,
    ) -> Result<(), crate::geojson::update::SourceUpdateError> {
        self.map_context.set_geojson_data(source_name, data)
    }

    /// Replaces a GeoJSON source's data with the inline document `geojson`.
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

    /// Applies a style change and drops or reloads what it leaves stale, as
    /// [`crate::context::MapContext::mutate_style`] does.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.map_context.mutate_style(apply)
    }
}

#[cfg(feature = "experimental")]
mod sky;
#[cfg(test)]
mod tests;

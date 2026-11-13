//! Bringing a running map's tiles and GPU buffers up to date with style and `feature-state` changes.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{StyleChange, StyleMutationError};
use crate::{
    context::MapContext,
    geojson::update::GeoJsonVersions,
    io::tile_retry::{self, RequestKind},
    style::{
        feature_state::{FeatureId, FeatureStateTarget},
        Style,
    },
    vector::upload_system::style_refresh::DirtyVectorStyle,
};

impl MapContext {
    /// Applies a style change: updates resident GPU buffers in-place for layer order, zoom bounds,
    /// visibility hiding, and paint edits, and fetches loaded vector tiles again only when geometry,
    /// filtering, or layout changed. GeoJSON data the change replaced, as with
    /// [`Style::set_geojson_data`], refreshes the tiles it changed in by itself.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        let geojson = GeoJsonVersions::of(&self.style);
        let change = apply(&mut self.style)?;
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if !change.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &change.removed_layers);
        }
        if !change.layers.is_empty() || !change.removed_layers.is_empty() {
            DirtyVectorStyle::mark_style_edit(&mut self.world, change.layers.iter().cloned());
        }
        if change.redraw_tiles {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        } else {
            geojson.refresh_changed(&self.style, &mut self.world);
        }
        for name in &change.removed_raster_sources {
            crate::raster::forget_source(&mut self.world, name);
        }
        for name in &change.reloaded_image_sources {
            crate::raster::image_source::reload(&mut self.world, &self.style, name);
        }
        Ok(change)
    }

    /// Merges `state` into the runtime state of feature `id` in `target`, refreshing resident
    /// GPU paint buffers, symbol placements, and rendered-feature queries in-place.
    pub fn set_feature_state(
        &mut self,
        target: FeatureStateTarget,
        id: impl Into<FeatureId>,
        state: Map<String, Value>,
    ) {
        let source = target.source.clone();
        self.style.set_feature_state(target, id, state);
        DirtyVectorStyle::mark_feature_state(&mut self.world, source);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Returns the runtime state map of feature `id` in `target`, if any.
    pub fn feature_state(
        &self,
        target: &FeatureStateTarget,
        id: impl Into<FeatureId>,
    ) -> Option<&BTreeMap<String, Value>> {
        self.style.feature_state(target, id)
    }

    /// Removes `key` (or all keys when `None`) from feature `id` (or all features of `target` when
    /// `id` is `None`), refreshing resident GPU buffers and rendered-feature queries in-place.
    pub fn remove_feature_state(
        &mut self,
        target: &FeatureStateTarget,
        id: Option<FeatureId>,
        key: Option<&str>,
    ) {
        self.style.remove_feature_state(target, id, key);
        DirtyVectorStyle::mark_feature_state(&mut self.world, target.source.clone());
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }
}

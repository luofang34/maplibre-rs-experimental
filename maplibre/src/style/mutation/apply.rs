//! Bringing a running map's tiles and GPU buffers up to date with style and `feature-state` changes.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{StyleChange, StyleMutationError};
use crate::{
    context::MapContext,
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
    /// filtering, or layout changed. GeoJSON data the change replaced, when the `geojson` feature
    /// is enabled, refreshes the tiles it changed in by itself.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        #[cfg(feature = "geojson")]
        let geojson = crate::geojson::update::GeoJsonVersions::of(&self.style);
        let change = apply(&mut self.style)?;
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if self.style.has_active_transitions() {
            crate::render::frame_signals::keep_animating(&mut self.world);
        }
        if !change.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &change.removed_layers);
        }
        if !change.layers.is_empty() || !change.removed_layers.is_empty() {
            DirtyVectorStyle::mark_style_edit(&mut self.world, change.layers.iter().cloned());
        }
        if change.redraw_tiles {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        } else {
            #[cfg(feature = "geojson")]
            geojson.refresh_changed(&self.style, &mut self.world);
        }
        #[cfg(feature = "raster")]
        {
            for name in &change.removed_raster_sources {
                crate::raster::forget_source(&mut self.world, name);
            }
            for name in &change.reloaded_image_sources {
                crate::raster::image_source::reload(&mut self.world, &self.style, name);
            }
        }
        Ok(change)
    }

    /// Replaces the style with `next` via [`Style::diff_and_replace`], retaining loaded tiles and
    /// source generations for unchanged sources while refreshing only affected layers and caches.
    pub fn set_style(&mut self, next: Style) -> crate::style::diff::StyleReplacementDiff {
        #[cfg(feature = "geojson")]
        let geojson = crate::geojson::update::GeoJsonVersions::of(&self.style);
        let diff = self.style.diff_and_replace(next);
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if self.style.has_active_transitions() {
            crate::render::frame_signals::keep_animating(&mut self.world);
        }
        if !diff.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &diff.removed_layers);
        }
        let affected = diff.affected_layers();
        if !affected.is_empty() || !diff.removed_layers.is_empty() || diff.global_state_changed {
            DirtyVectorStyle::mark_style_edit(&mut self.world, affected);
        }
        if diff.redraw_tiles {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        } else {
            #[cfg(feature = "geojson")]
            geojson.refresh_changed(&self.style, &mut self.world);
        }
        #[cfg(feature = "raster")]
        {
            for name in &diff.removed_raster_sources {
                crate::raster::forget_source(&mut self.world, name);
            }
            for name in &diff.reloaded_image_sources {
                crate::raster::image_source::reload(&mut self.world, &self.style, name);
            }
            if diff.redraw_raster_tiles || diff.terrain_changed {
                tile_retry::refresh(&mut self.world, RequestKind::Raster);
            }
        }
        if diff.terrain_changed || diff.projection_changed {
            crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        }
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

//! Applying style diffs, style replacements, and global-state edits onto a running [`MapContext`].

use std::collections::BTreeMap;

use rustc_hash::FxHashMap as HashMap;
use serde_json::{Map, Value};

use crate::{
    context::MapContext,
    io::tile_retry::{self, RequestKind},
    style::{
        diff::StyleReplacementDiff,
        feature_state::{FeatureId, FeatureStateTarget},
        layer::{classify::needs_tile_redraw, from_vector_tiles, StyleLayer},
        mutation::{StyleChange, StyleMutationError},
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
        crate::vector::upload_system::dynamic_points::flush_deferred_geojson_bytes(
            &mut self.style,
            &mut self.world,
        );
        #[cfg(feature = "geojson")]
        let geojson = crate::geojson::update::GeoJsonVersions::of(&self.style);
        let old_zooms = snapshot_layer_zooms(&self.style);
        let change = apply(&mut self.style)?;
        self.style.bump_revision();
        #[cfg(feature = "geojson")]
        crate::vector::upload_system::dynamic_points::invalidate_after_style_change(
            &self.style,
            &mut self.world,
        );
        let widened = zoom_bounds_widened(&old_zooms, &self.style);
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if self.style.has_active_transitions() {
            crate::render::frame_signals::keep_animating(&mut self.world);
        }
        if !change.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &change.removed_layers);
            if let Some(registry) = self
                .world
                .resources
                .get_mut::<crate::custom_layer::CustomLayerRegistry>()
            {
                registry.deinitialize_layers(&change.removed_layers);
            }
        }
        if !change.layers.is_empty() || !change.removed_layers.is_empty() {
            DirtyVectorStyle::mark_style_edit(&mut self.world, change.layers.iter().cloned());
        }
        if change.redraw_tiles || widened {
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
    pub fn set_style(&mut self, next: Style) -> StyleReplacementDiff {
        #[cfg(feature = "geojson")]
        crate::vector::upload_system::dynamic_points::flush_deferred_geojson_bytes(
            &mut self.style,
            &mut self.world,
        );
        #[cfg(feature = "geojson")]
        let geojson = crate::geojson::update::GeoJsonVersions::of(&self.style);
        let old_zooms = snapshot_layer_zooms(&self.style);
        let had_dem = crate::terrain::source::dem_source(&self.style).is_some();
        let diff = self.style.diff_and_replace(next);
        #[cfg(feature = "geojson")]
        crate::vector::upload_system::dynamic_points::invalidate_after_style_change(
            &self.style,
            &mut self.world,
        );
        let widened = zoom_bounds_widened(&old_zooms, &self.style);
        let enabled_dem = !had_dem && crate::terrain::source::dem_source(&self.style).is_some();
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if self.style.has_active_transitions() {
            crate::render::frame_signals::keep_animating(&mut self.world);
        }
        if !diff.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &diff.removed_layers);
            if let Some(registry) = self
                .world
                .resources
                .get_mut::<crate::custom_layer::CustomLayerRegistry>()
            {
                registry.deinitialize_layers(&diff.removed_layers);
            }
        }
        let affected = diff.affected_layers();
        if !affected.is_empty() || !diff.removed_layers.is_empty() || diff.global_state_changed {
            DirtyVectorStyle::mark_style_edit(&mut self.world, affected);
        }
        if diff.redraw_tiles || widened || (cfg!(target_arch = "wasm32") && enabled_dem) {
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

    /// Sets a global state key, updating paint-only layer dependencies in-place on resident GPU
    /// buffers and refetching loaded vector tiles only when layout, filter, or geometry changed.
    pub fn set_global_state(&mut self, key: &str, value: Value) {
        let before: HashMap<String, StyleLayer> = self
            .style
            .layers
            .iter()
            .map(|layer| (layer.id.clone(), layer.clone()))
            .collect();
        let changed = self.style.set_global_state(key, value);
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if changed.is_empty() {
            return;
        }
        let needs_redraw = self.style.layers.iter().any(|after| {
            changed.contains(&after.id)
                && before
                    .get(&after.id)
                    .is_none_or(|prev| needs_tile_redraw(prev, after))
        });
        DirtyVectorStyle::mark_style_edit(&mut self.world, changed);
        if needs_redraw {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        }
    }
}

fn snapshot_layer_zooms(style: &Style) -> HashMap<String, (Option<f64>, Option<f64>)> {
    style
        .layers
        .iter()
        .filter(|layer| from_vector_tiles(layer) && !layer.is_hidden())
        .map(|layer| (layer.id.clone(), (layer.minzoom, layer.maxzoom)))
        .collect()
}

fn zoom_bounds_widened(
    old_zooms: &HashMap<String, (Option<f64>, Option<f64>)>,
    style: &Style,
) -> bool {
    style.layers.iter().any(|layer| {
        if !from_vector_tiles(layer) || layer.is_hidden() {
            return false;
        }
        let Some(&(old_min, old_max)) = old_zooms.get(&layer.id) else {
            return false;
        };
        let min_widened = match (old_min, layer.minzoom) {
            (Some(old), None) => old.floor() > 0.0,
            (Some(old), Some(new)) => new.floor() < old.floor(),
            _ => false,
        };
        let max_widened = match (old_max, layer.maxzoom) {
            (Some(_), None) => true,
            (Some(old), Some(new)) => new.ceil() > old.ceil(),
            _ => false,
        };
        min_widened || max_widened
    })
}

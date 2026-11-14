//! Diffing whole-style replacement (`setStyle` / [`Style::diff_and_replace`]).
//!
//! Comparing an incoming [`Style`] against the running style preserves source `generation` IDs
//! and resident tiles for unchanged sources, updates GPU paint and layer order in-place when
//! layout/filtering did not change, and initiates configured paint transitions on retained layers.

use std::collections::{HashMap, HashSet};

use super::{
    layer::StyleLayer,
    mutation::{
        classify::{needs_tile_redraw, push_unique},
        from_vector_tiles,
    },
    source::{fresh_generation, CanvasSource, GeoJsonSource, ImageSource, Source, VideoSource},
    transitions::{
        default_paint_json, interpolate_paint_value, is_transitionable_property, ActiveTransition,
        TransitionSpec,
    },
    Style,
};

/// Minimal update plan produced when replacing one [`Style`] with another via [`Style::diff_and_replace`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleReplacementDiff {
    /// Source names newly introduced in the replacement style.
    pub added_sources: Vec<String>,
    /// Source names removed from the previous style.
    pub removed_sources: Vec<String>,
    /// Retained source names whose definition changed.
    pub updated_sources: Vec<String>,
    /// Layer IDs newly introduced in the replacement style.
    pub added_layers: Vec<String>,
    /// Layer IDs removed from the previous style.
    pub removed_layers: Vec<String>,
    /// Retained layer IDs whose paint, layout, filter, visibility or zoom bounds changed.
    pub updated_layers: Vec<String>,
    /// Retained layer IDs whose painter index changed.
    pub reordered_layers: Vec<String>,
    /// Whether vector tiles must be fetched and tessellated again.
    pub redraw_tiles: bool,
    /// Whether tiled raster or DEM sources were added or reconfigured.
    pub redraw_raster_tiles: bool,
    /// Image/video/canvas sources whose corners or image URL changed.
    pub reloaded_image_sources: Vec<String>,
    /// Raster, DEM, image, video or canvas sources whose resident GPU textures must be dropped.
    pub removed_raster_sources: Vec<String>,
    /// Whether the root `terrain` specification changed.
    pub terrain_changed: bool,
    /// Whether the root `sky` specification changed.
    pub sky_changed: bool,
    /// Whether the root `light` specification changed.
    pub light_changed: bool,
    /// Whether the root `projection` specification changed.
    pub projection_changed: bool,
    /// Whether `sprite` or host `images` changed.
    pub sprite_changed: bool,
    /// Whether the `glyphs` font URL template changed.
    pub glyphs_changed: bool,
    /// Whether the root `transition` specification changed.
    pub transition_changed: bool,
    /// Whether resolved `global-state` values changed.
    pub global_state_changed: bool,
}

impl StyleReplacementDiff {
    /// Deduplicated list of added, updated and reordered layer IDs for in-place GPU style refresh.
    pub fn affected_layers(&self) -> Vec<String> {
        let mut out = Vec::new();
        for id in self
            .added_layers
            .iter()
            .chain(&self.updated_layers)
            .chain(&self.reordered_layers)
        {
            push_unique(&mut out, id);
        }
        out
    }
}

fn geojson_definition_eq(a: &GeoJsonSource, b: &GeoJsonSource) -> bool {
    a.data == b.data
        && a.maxzoom == b.maxzoom
        && a.minzoom == b.minzoom
        && a.buffer == b.buffer
        && a.tolerance == b.tolerance
        && a.line_metrics == b.line_metrics
        && a.promote_id == b.promote_id
        && a.generate_id == b.generate_id
        && a.cluster == b.cluster
        && a.cluster_radius == b.cluster_radius
        && a.cluster_max_zoom == b.cluster_max_zoom
        && a.cluster_min_points == b.cluster_min_points
        && a.cluster_properties == b.cluster_properties
        && a.filter == b.filter
        && a.volatile == b.volatile
}

fn image_definition_eq(a: &ImageSource, b: &ImageSource) -> bool {
    a.url == b.url && a.coordinates == b.coordinates
}

fn video_definition_eq(a: &VideoSource, b: &VideoSource) -> bool {
    a.urls == b.urls && a.coordinates == b.coordinates
}

fn canvas_definition_eq(a: &CanvasSource, b: &CanvasSource) -> bool {
    a.canvas == b.canvas && a.animate == b.animate && a.coordinates == b.coordinates
}

fn is_raster_like(source: &Source) -> bool {
    matches!(
        source,
        Source::Raster(_)
            | Source::RasterDem(_)
            | Source::Image(_)
            | Source::Video(_)
            | Source::Canvas(_)
    )
}

fn renumber_layers(layers: &mut [StyleLayer]) {
    for (position, layer) in layers.iter_mut().enumerate() {
        layer.index = u32::try_from(position).unwrap_or(u32::MAX);
    }
}

impl Style {
    /// Diffs `next` against `self`, preserving unchanged source generations and resident tile
    /// state while computing minimal layer, source and root-property updates, then replaces `self`.
    pub fn diff_and_replace(&mut self, mut next: Style) -> StyleReplacementDiff {
        renumber_layers(&mut next.layers);
        next.resolve_global_state();
        next.active_transitions =
            super::transitions::ActiveTransitions::with_clock(self.active_transitions.clock_ms());

        let mut diff = StyleReplacementDiff {
            terrain_changed: self.terrain != next.terrain,
            sky_changed: serde_json::to_value(&self.sky).ok()
                != serde_json::to_value(&next.sky).ok(),
            light_changed: serde_json::to_value(&self.light).ok()
                != serde_json::to_value(&next.light).ok(),
            projection_changed: self.projection != next.projection,
            sprite_changed: self.sprite != next.sprite || self.images != next.images,
            glyphs_changed: self.glyphs != next.glyphs,
            transition_changed: self.transition != next.transition,
            global_state_changed: self.global_state_values() != next.global_state_values(),
            ..Default::default()
        };

        let mut changed_vector_sources = HashSet::new();

        for (name, old_source) in &self.sources {
            if !next.sources.contains_key(name) {
                diff.removed_sources.push(name.clone());
                if is_raster_like(old_source) {
                    diff.removed_raster_sources.push(name.clone());
                }
            }
        }

        for (name, new_source) in &mut next.sources {
            let Some(old_source) = self.sources.get(name) else {
                match new_source {
                    Source::GeoJson(g) => g.generation = fresh_generation(),
                    Source::Image(i) => i.generation = fresh_generation(),
                    Source::Video(v) => v.generation = fresh_generation(),
                    Source::Canvas(c) => c.generation = fresh_generation(),
                    Source::Raster(_) | Source::RasterDem(_) => {
                        diff.redraw_raster_tiles = true;
                    }
                    Source::Vector(_) => {}
                }
                diff.added_sources.push(name.clone());
                continue;
            };

            match (old_source, new_source) {
                (Source::Vector(a), Source::Vector(b)) => {
                    if a != b {
                        diff.updated_sources.push(name.clone());
                        changed_vector_sources.insert(name.clone());
                    }
                }
                (Source::Raster(a), Source::Raster(b)) => {
                    if a != b {
                        diff.updated_sources.push(name.clone());
                        push_unique(&mut diff.removed_raster_sources, name);
                        diff.redraw_raster_tiles = true;
                    }
                }
                (Source::RasterDem(a), Source::RasterDem(b)) => {
                    if a != b {
                        diff.updated_sources.push(name.clone());
                        push_unique(&mut diff.removed_raster_sources, name);
                        diff.redraw_raster_tiles = true;
                    }
                }
                (Source::GeoJson(a), Source::GeoJson(b)) => {
                    if geojson_definition_eq(a, b) {
                        b.generation = a.generation;
                    } else {
                        b.generation = fresh_generation();
                        diff.updated_sources.push(name.clone());
                    }
                }
                (Source::Image(a), Source::Image(b)) => {
                    if image_definition_eq(a, b) {
                        b.generation = a.generation;
                    } else {
                        b.generation = fresh_generation();
                        diff.updated_sources.push(name.clone());
                        push_unique(&mut diff.reloaded_image_sources, name);
                    }
                }
                (Source::Video(a), Source::Video(b)) => {
                    if video_definition_eq(a, b) {
                        if b.frame.is_none() {
                            b.frame = a.frame.clone();
                        }
                        b.generation = a.generation;
                    } else {
                        if b.frame.is_none() && a.urls == b.urls {
                            b.frame = a.frame.clone();
                        }
                        b.generation = fresh_generation();
                        diff.updated_sources.push(name.clone());
                        push_unique(&mut diff.reloaded_image_sources, name);
                    }
                }
                (Source::Canvas(a), Source::Canvas(b)) => {
                    if canvas_definition_eq(a, b) {
                        if b.frame.is_none() {
                            b.frame = a.frame.clone();
                        }
                        b.generation = a.generation;
                    } else {
                        if b.frame.is_none() && a.canvas == b.canvas {
                            b.frame = a.frame.clone();
                        }
                        b.generation = fresh_generation();
                        diff.updated_sources.push(name.clone());
                        push_unique(&mut diff.reloaded_image_sources, name);
                    }
                }
                (old_other, new_other) => {
                    diff.updated_sources.push(name.clone());
                    if is_raster_like(old_other) {
                        push_unique(&mut diff.removed_raster_sources, name);
                    }
                    match new_other {
                        Source::GeoJson(g) => g.generation = fresh_generation(),
                        Source::Image(i) => {
                            i.generation = fresh_generation();
                            push_unique(&mut diff.reloaded_image_sources, name);
                        }
                        Source::Video(v) => {
                            v.generation = fresh_generation();
                            push_unique(&mut diff.reloaded_image_sources, name);
                        }
                        Source::Canvas(c) => {
                            c.generation = fresh_generation();
                            push_unique(&mut diff.reloaded_image_sources, name);
                        }
                        Source::Raster(_) | Source::RasterDem(_) => {
                            diff.redraw_raster_tiles = true;
                        }
                        Source::Vector(_) => {
                            changed_vector_sources.insert(name.clone());
                        }
                    }
                }
            }
        }

        diff.added_sources.sort();
        diff.removed_sources.sort();
        diff.updated_sources.sort();
        diff.removed_raster_sources.sort();
        diff.reloaded_image_sources.sort();

        // Preserve runtime feature-state for sources that remain unchanged unless `next` supplied its own.
        if next.feature_state.is_empty() && !self.feature_state.is_empty() {
            let mut preserved = self.feature_state.clone();
            for removed in &diff.removed_sources {
                let target = super::FeatureStateTarget {
                    source: removed.clone(),
                    source_layer: None,
                };
                preserved.remove(&target, None, None);
            }
            next.feature_state = preserved;
        }

        let old_layers: HashMap<&str, &StyleLayer> = self
            .layers
            .iter()
            .map(|layer| (layer.id.as_str(), layer))
            .collect();
        let next_layer_ids: HashSet<&str> =
            next.layers.iter().map(|layer| layer.id.as_str()).collect();

        for old_layer in &self.layers {
            if !next_layer_ids.contains(old_layer.id.as_str()) {
                diff.removed_layers.push(old_layer.id.clone());
            }
        }

        let mut pending_transitions: Vec<ActiveTransition> = Vec::new();
        for new_layer in &next.layers {
            let Some(old_layer) = old_layers.get(new_layer.id.as_str()).copied() else {
                diff.added_layers.push(new_layer.id.clone());
                if from_vector_tiles(new_layer) && !new_layer.is_hidden() {
                    diff.redraw_tiles = true;
                }
                continue;
            };

            if old_layer.index != new_layer.index {
                diff.reordered_layers.push(new_layer.id.clone());
            }

            let source_changed = new_layer
                .source
                .as_deref()
                .is_some_and(|src| changed_vector_sources.contains(src));
            if source_changed || needs_tile_redraw(old_layer, new_layer) {
                diff.redraw_tiles = true;
                diff.updated_layers.push(new_layer.id.clone());
                continue;
            }

            let paint_changed = !old_layer.same_paint(new_layer);
            let meta_or_bounds_changed = old_layer.visibility != new_layer.visibility
                || old_layer.minzoom != new_layer.minzoom
                || old_layer.maxzoom != new_layer.maxzoom
                || old_layer.metadata != new_layer.metadata;

            if paint_changed || meta_or_bounds_changed {
                diff.updated_layers.push(new_layer.id.clone());
            }

            if paint_changed && old_layer.type_ == new_layer.type_ {
                collect_layer_transitions(
                    old_layer,
                    new_layer,
                    next.transition,
                    &mut pending_transitions,
                );
            }
        }

        if diff.projection_changed && next.layers.iter().any(from_vector_tiles) {
            diff.redraw_tiles = true;
        }
        if (diff.glyphs_changed || diff.sprite_changed)
            && next
                .layers
                .iter()
                .any(|layer| !layer.is_hidden() && layer.type_ == "symbol")
        {
            diff.redraw_tiles = true;
        }

        // Start any active paint transitions collected for retained layers at progress 0.
        for entry in pending_transitions {
            if let Some(initial) = interpolate_paint_value(&entry.from, &entry.to, 0.0) {
                let _ = next.set_paint_property_immediate(&entry.layer, &entry.property, initial);
                push_unique(&mut diff.updated_layers, &entry.layer);
                let _ = next.apply_paint_with_transition(&entry.layer, &entry.property, entry.to);
            }
        }

        *self = next;
        diff
    }
}

fn collect_layer_transitions(
    old_layer: &StyleLayer,
    new_layer: &StyleLayer,
    root_transition: Option<TransitionSpec>,
    out: &mut Vec<ActiveTransition>,
) {
    let (Ok(old_maps), Ok(new_maps)) = (old_layer.property_maps(), new_layer.property_maps())
    else {
        return;
    };
    for (prop, to_val) in &new_maps.paint {
        if !is_transitionable_property(prop) {
            continue;
        }
        let timing = new_layer.transitions.get(prop).copied().or(root_transition);
        if !timing.is_some_and(TransitionSpec::is_active) {
            continue;
        }
        let Some(from_val) = old_maps
            .paint
            .get(prop)
            .cloned()
            .or_else(|| default_paint_json(old_layer, prop))
        else {
            continue;
        };
        if interpolate_paint_value(&from_val, to_val, 0.5).is_some()
            && interpolate_paint_value(&from_val, to_val, 0.0)
                != interpolate_paint_value(&from_val, to_val, 1.0)
        {
            if let Some(spec) = timing {
                out.push(ActiveTransition {
                    layer: new_layer.id.clone(),
                    property: prop.clone(),
                    from: from_val,
                    to: to_val.clone(),
                    timing: spec,
                    started_ms: 0.0,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;

//! Changing a loaded style: layers, their properties and their sources.
//!
//! Every change is checked before it is applied and leaves the style untouched when it fails.
//! Edits are made to a layer as declared, so a layer that reads global state keeps doing so. The
//! returned [`StyleChange`] says which layers changed and whether tiles must be fetched again;
//! per-tile buffers bake in draw order, colors and filters, so only layers drawn from vector
//! tiles need that, while background and raster layers are read from the style every frame.
//! Until the fetched tiles arrive, a moved layer can be drawn in its old order relative to layers
//! that read the live order.

use serde_json::{json, Value};

use super::{
    layer::StyleLayer,
    source::{fresh_generation, Source},
    Style,
};

mod types;
pub use types::{StyleChange, StyleMutationError};

/// Whether a layer's content lives in per-tile buffers built from vector tiles.
pub(crate) fn from_vector_tiles(layer: &StyleLayer) -> bool {
    layer.type_ != "background"
        && layer.type_ != "custom"
        && !crate::io::tile_sources::RASTER_LAYER_TYPES.contains(&layer.type_.as_str())
}

impl Style {
    /// Adds or replaces an image that symbol layers can draw by name. Every symbol layer is
    /// reported as changed, since any of them may name it.
    pub fn add_image(&mut self, name: &str, image: super::StyleImage) -> StyleChange {
        self.images.insert(name.to_owned(), image);
        self.symbol_layers_changed()
    }

    /// Removes an added image; layers that named it lose the icon.
    pub fn remove_image(&mut self, name: &str) -> StyleChange {
        self.images.remove(name);
        self.symbol_layers_changed()
    }

    fn symbol_layers_changed(&self) -> StyleChange {
        let layers = self
            .layers
            .iter()
            .filter(|layer| layer.type_ == "symbol")
            .map(|layer| layer.id.clone())
            .collect();
        self.finish(StyleChange {
            layers,
            ..StyleChange::default()
        })
    }

    /// Adds a layer given as style JSON above the layer `before`, or on top when `None`.
    pub fn add_layer(
        &mut self,
        layer: Value,
        before: Option<&str>,
    ) -> Result<StyleChange, StyleMutationError> {
        let id = layer
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let parsed: StyleLayer =
            serde_json::from_value(layer).map_err(|source| StyleMutationError::InvalidLayer {
                layer: id.clone(),
                source,
            })?;
        if self.layers.iter().any(|existing| existing.id == parsed.id) {
            return Err(StyleMutationError::DuplicateLayer { layer: id });
        }
        if let Some(source_name) = &parsed.source {
            if !self.sources.contains_key(source_name) {
                return Err(StyleMutationError::MissingSource {
                    layer: id,
                    source_name: source_name.clone(),
                });
            }
        }
        let position = self.position_before(before)?;
        self.ensure_supported(&parsed)?;
        self.ensure_shape(&parsed)?;
        let needs_tessellation = from_vector_tiles(&parsed) && !parsed.is_hidden();
        self.layers.insert(position, parsed);
        let mut change = self.renumber();
        push_unique(&mut change.layers, &id);
        self.resolve_global_state();
        change.redraw_tiles = needs_tessellation;
        Ok(change)
    }

    /// Removes a layer.
    pub fn remove_layer(&mut self, id: &str) -> Result<StyleChange, StyleMutationError> {
        let position = self.position_of(id)?;
        let _removed = self.layers.remove(position);
        self.active_transitions.remove_layer(id);
        let mut change = self.renumber();
        push_unique(&mut change.layers, id);
        self.resolve_global_state();
        change.removed_layers.push(id.to_owned());
        Ok(change)
    }

    /// Moves a layer above the layer `before`, or to the top when `None`.
    pub fn move_layer(
        &mut self,
        id: &str,
        before: Option<&str>,
    ) -> Result<StyleChange, StyleMutationError> {
        let position = self.position_of(id)?;
        if before == Some(id) {
            return Ok(StyleChange::default());
        }
        let layer = self.layers.remove(position);
        let target = match self.position_before(before) {
            Ok(target) => target,
            Err(error) => {
                self.layers.insert(position, layer);
                return Err(error);
            }
        };
        self.layers.insert(target, layer);
        Ok(self.renumber())
    }

    /// Sets a paint property; `null` restores its default. When the layer or root style declares
    /// an active `transition` for `name`, starts an in-flight paint transition.
    pub fn set_paint_property(
        &mut self,
        layer: &str,
        name: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        self.apply_paint_with_transition(layer, name, value)
    }

    pub(crate) fn set_paint_property_immediate(
        &mut self,
        layer: &str,
        name: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        self.edit_layer(layer, |document| set_member(document, "paint", name, value))
    }

    /// Sets a layout property, including `visibility`; `null` restores its default.
    pub fn set_layout_property(
        &mut self,
        layer: &str,
        name: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        self.edit_layer(layer, |document| {
            set_member(document, "layout", name, value)
        })
    }

    /// Sets a layer's filter; `None` removes it.
    pub fn set_filter(
        &mut self,
        layer: &str,
        filter: Option<Value>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.edit_layer(layer, |document| match (document.as_object_mut(), filter) {
            (Some(map), Some(filter)) => {
                map.insert("filter".into(), filter);
            }
            (Some(map), None) => {
                map.remove("filter");
            }
            _ => {}
        })
    }

    /// Sets the zooms between which a layer is drawn; `None` removes a bound.
    pub fn set_layer_zoom_range(
        &mut self,
        layer: &str,
        minzoom: Option<f64>,
        maxzoom: Option<f64>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.edit_layer(layer, |document| {
            if let Some(map) = document.as_object_mut() {
                for (key, bound) in [("minzoom", minzoom), ("maxzoom", maxzoom)] {
                    match bound {
                        Some(bound) => map.insert(key.into(), json!(bound)),
                        None => map.remove(key),
                    };
                }
            }
        })
    }

    /// Adds a source. A GeoJSON source gets a generation of its own, so a source removed and
    /// added again under one name never shows the data it had before.
    pub fn add_source(
        &mut self,
        name: &str,
        mut source: Source,
    ) -> Result<StyleChange, StyleMutationError> {
        if self.sources.contains_key(name) {
            return Err(StyleMutationError::DuplicateSource {
                source_name: name.to_owned(),
            });
        }
        let url_only = match &source {
            Source::Vector(vector) | Source::Raster(vector) => {
                vector.tiles.is_none() && vector.url.is_some()
            }
            Source::RasterDem(dem) => dem.tiles.is_none() && dem.url.is_some(),
            Source::GeoJson(_) | Source::Image(_) | Source::Video(_) | Source::Canvas(_) => false,
        };
        if url_only {
            return Err(StyleMutationError::TileJsonNotSupported {
                source_name: name.to_owned(),
            });
        }
        match &mut source {
            Source::GeoJson(geojson) => geojson.generation = fresh_generation(),
            Source::Image(image) => image.generation = fresh_generation(),
            Source::Video(video) => video.generation = fresh_generation(),
            Source::Canvas(canvas) => canvas.generation = fresh_generation(),
            Source::Vector(_) | Source::Raster(_) | Source::RasterDem(_) => {}
        }
        self.sources.insert(name.to_owned(), source);
        Ok(StyleChange::default())
    }

    /// Removes a source no layer draws from.
    pub fn remove_source(&mut self, name: &str) -> Result<StyleChange, StyleMutationError> {
        if !self.sources.contains_key(name) {
            return Err(StyleMutationError::UnknownSource {
                source_name: name.to_owned(),
            });
        }
        if let Some(layer) = self
            .layers
            .iter()
            .find(|layer| layer.source.as_deref() == Some(name))
        {
            return Err(StyleMutationError::SourceInUse {
                source_name: name.to_owned(),
                layer: layer.id.clone(),
            });
        }
        let removed = self.sources.remove(name);
        Ok(StyleChange {
            removed_raster_sources: matches!(
                removed,
                Some(
                    Source::Raster(_)
                        | Source::RasterDem(_)
                        | Source::Image(_)
                        | Source::Video(_)
                        | Source::Canvas(_)
                )
            )
            .then(|| name.to_owned())
            .into_iter()
            .collect(),
            ..StyleChange::default()
        })
    }

    fn position_of(&self, id: &str) -> Result<usize, StyleMutationError> {
        self.layers
            .iter()
            .position(|layer| layer.id == id)
            .ok_or_else(|| StyleMutationError::UnknownLayer {
                layer: id.to_owned(),
            })
    }

    /// Where a layer added above `before` goes: the end when `None`.
    fn position_before(&self, before: Option<&str>) -> Result<usize, StyleMutationError> {
        match before {
            Some(id) => self.position_of(id),
            None => Ok(self.layers.len()),
        }
    }

    fn ensure_supported(&self, layer: &StyleLayer) -> Result<(), StyleMutationError> {
        let findings = self.validate_layer(layer);
        if findings.is_empty() {
            Ok(())
        } else {
            Err(StyleMutationError::Unsupported {
                layer: layer.id.clone(),
                findings,
            })
        }
    }

    /// Gives every layer the index of its position; returns the layers whose index changed.
    fn renumber(&mut self) -> StyleChange {
        let mut change = StyleChange::default();
        for (position, layer) in self.layers.iter_mut().enumerate() {
            let index = u32::try_from(position).unwrap_or(u32::MAX);
            if layer.index != index {
                layer.index = index;
                change.layers.push(layer.id.clone());
            }
        }
        change
    }

    fn finish(&self, mut change: StyleChange) -> StyleChange {
        change.redraw_tiles |= change.layers.iter().any(|id| {
            self.layers
                .iter()
                .find(|layer| &layer.id == id)
                .is_some_and(from_vector_tiles)
        });
        change
    }

    fn edit_layer(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut Value),
    ) -> Result<StyleChange, StyleMutationError> {
        let position = self.position_of(id)?;
        let mut document =
            serde_json::to_value(self.declared_layer_at(position)).map_err(|source| {
                StyleMutationError::InvalidLayer {
                    layer: id.to_owned(),
                    source,
                }
            })?;
        edit(&mut document);
        let mut edited: StyleLayer = serde_json::from_value(document).map_err(|source| {
            StyleMutationError::InvalidLayer {
                layer: id.to_owned(),
                source,
            }
        })?;
        edited.index = self.layers[position].index;
        self.ensure_supported(&edited)?;
        // Only the zoom range is checked: an edit does not change the layer's source, so a layer
        // loaded in a legacy shape stays editable.
        ensure_zoom_range(&edited)?;
        let before_layer = self.layers[position].clone();
        let before = serde_json::to_value(&before_layer).ok();
        self.layers[position] = edited;
        self.state_templates.remove(id);
        self.resolve_global_state();
        let after = serde_json::to_value(&self.layers[position]).ok();
        Ok(if before == after {
            StyleChange::default()
        } else {
            StyleChange {
                layers: vec![id.to_owned()],
                redraw_tiles: classify::needs_tile_redraw(&before_layer, &self.layers[position]),
                ..Default::default()
            }
        })
    }
}

mod apply;
pub(crate) mod classify;
mod image_source;
mod media_source;
mod shape;

use classify::{push_unique, set_member};
use shape::ensure_zoom_range;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

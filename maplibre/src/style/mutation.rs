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
use thiserror::Error;

use super::{
    layer::StyleLayer,
    source::{fresh_generation, Source},
    validation::StyleValidationError,
    Style,
};
use crate::{
    context::MapContext,
    io::tile_retry::{self, RequestKind},
};

/// Why a change was refused.
#[derive(Debug, Error)]
pub enum StyleMutationError {
    /// A layer with this id exists already.
    #[error("style already has a layer `{layer}`")]
    DuplicateLayer {
        /// The clashing id.
        layer: String,
    },
    /// The style has no layer with this id.
    #[error("style has no layer `{layer}`")]
    UnknownLayer {
        /// The requested id.
        layer: String,
    },
    /// A source with this name exists already.
    #[error("style already has a source `{source_name}`")]
    DuplicateSource {
        /// The clashing name.
        source_name: String,
    },
    /// The style has no source with this name.
    #[error("style has no source `{source_name}`")]
    UnknownSource {
        /// The requested name.
        source_name: String,
    },
    /// A layer names a source the style does not have.
    #[error("layer `{layer}` uses source `{source_name}`, which the style does not have")]
    MissingSource {
        /// The layer.
        layer: String,
        /// The absent source.
        source_name: String,
    },
    /// A layer still draws from the source.
    #[error("source `{source_name}` is used by layer `{layer}`")]
    SourceInUse {
        /// The source.
        source_name: String,
        /// A layer using it.
        layer: String,
    },
    /// A layer that draws from a source names none.
    #[error("layer `{layer}` needs a source")]
    SourceRequired {
        /// The layer.
        layer: String,
    },
    /// A layer on a vector source names no source layer.
    #[error("layer `{layer}` needs a source-layer of vector source `{source_name}`")]
    SourceLayerRequired {
        /// The layer.
        layer: String,
        /// The vector source.
        source_name: String,
    },
    /// The layer's type cannot draw from the kind of source it names.
    #[error("layer `{layer}` of type `{kind}` cannot draw from source `{source_name}`")]
    WrongSourceKind {
        /// The layer.
        layer: String,
        /// The layer's type.
        kind: String,
        /// The source it names.
        source_name: String,
    },
    /// The lower zoom bound is above the upper one.
    #[error("layer `{layer}` has minzoom {minzoom} above maxzoom {maxzoom}")]
    InvalidZoomRange {
        /// The layer.
        layer: String,
        /// The lower bound.
        minzoom: f64,
        /// The upper bound.
        maxzoom: f64,
    },
    /// A vector, raster or DEM source given only as a TileJSON url, which is not fetched here.
    #[error("source `{source_name}` gives only a TileJSON url; list its tiles instead")]
    TileJsonNotSupported {
        /// The source.
        source_name: String,
    },
    /// The layer, or the layer with the change applied, cannot be read.
    #[error("layer `{layer}` is invalid")]
    InvalidLayer {
        /// The layer.
        layer: String,
        /// Why it cannot be read.
        #[source]
        source: serde_json::Error,
    },
    /// The layer uses something this renderer does not support.
    #[error("layer `{layer}` is not supported: {}", findings.first().map(ToString::to_string).unwrap_or_default())]
    Unsupported {
        /// The layer.
        layer: String,
        /// Everything that is not supported.
        findings: Vec<StyleValidationError>,
    },
}

/// What a change did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleChange {
    /// Layers that were added, removed, edited or moved.
    pub layers: Vec<String>,
    /// Whether loaded vector tiles must be fetched again to show it.
    pub redraw_tiles: bool,
    /// Layers that are gone, whose leftovers in loaded tiles must be dropped.
    pub removed_layers: Vec<String>,
}

/// Whether a layer's content lives in per-tile buffers built from vector tiles.
pub(crate) fn from_vector_tiles(layer: &StyleLayer) -> bool {
    layer.type_ != "background"
        && !crate::io::tile_sources::RASTER_LAYER_TYPES.contains(&layer.type_.as_str())
}

impl Style {
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
        self.layers.insert(position, parsed);
        let mut change = self.renumber();
        push_unique(&mut change.layers, &id);
        self.resolve_global_state();
        Ok(self.finish(change))
    }

    /// Removes a layer.
    pub fn remove_layer(&mut self, id: &str) -> Result<StyleChange, StyleMutationError> {
        let position = self.position_of(id)?;
        let removed = self.layers.remove(position);
        let mut change = self.renumber();
        push_unique(&mut change.layers, id);
        self.resolve_global_state();
        change.redraw_tiles |= from_vector_tiles(&removed);
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
        let change = self.renumber();
        Ok(self.finish(change))
    }

    /// Sets a paint property; `null` restores its default.
    pub fn set_paint_property(
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
            Source::GeoJson(_) => false,
        };
        if url_only {
            return Err(StyleMutationError::TileJsonNotSupported {
                source_name: name.to_owned(),
            });
        }
        if let Source::GeoJson(geojson) = &mut source {
            geojson.generation = fresh_generation();
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
        self.sources.remove(name);
        Ok(StyleChange::default())
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

    /// Checks what GL JS checks of a layer: that its source exists, is of a kind its type can draw
    /// from and, for vector sources, is read through a source layer, and that the zoom range is
    /// ordered.
    fn ensure_shape(&self, layer: &StyleLayer) -> Result<(), StyleMutationError> {
        let id = layer.id.clone();
        if let (Some(minzoom), Some(maxzoom)) = (layer.minzoom, layer.maxzoom) {
            if minzoom > maxzoom {
                return Err(StyleMutationError::InvalidZoomRange {
                    layer: id,
                    minzoom,
                    maxzoom,
                });
            }
        }
        if layer.type_ == "background" {
            return Ok(());
        }
        let Some(source_name) = &layer.source else {
            return Err(StyleMutationError::SourceRequired { layer: id });
        };
        let Some(source) = self.sources.get(source_name) else {
            return Err(StyleMutationError::MissingSource {
                layer: id,
                source_name: source_name.clone(),
            });
        };
        let fits = match layer.type_.as_str() {
            "raster" => matches!(source, Source::Raster(_) | Source::RasterDem(_)),
            "hillshade" | "color-relief" => matches!(source, Source::RasterDem(_)),
            _ => matches!(source, Source::Vector(_) | Source::GeoJson(_)),
        };
        if !fits {
            return Err(StyleMutationError::WrongSourceKind {
                layer: id,
                kind: layer.type_.clone(),
                source_name: source_name.clone(),
            });
        }
        if matches!(source, Source::Vector(_)) && layer.source_layer.is_none() {
            return Err(StyleMutationError::SourceLayerRequired {
                layer: id,
                source_name: source_name.clone(),
            });
        }
        Ok(())
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
                change.redraw_tiles |= from_vector_tiles(layer);
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
        self.ensure_shape(&edited)?;
        let before = serde_json::to_value(&self.layers[position]).ok();
        self.layers[position] = edited;
        self.state_templates.remove(id);
        self.resolve_global_state();
        let after = serde_json::to_value(&self.layers[position]).ok();
        Ok(if before == after {
            StyleChange::default()
        } else {
            self.finish(StyleChange {
                layers: vec![id.to_owned()],
                ..Default::default()
            })
        })
    }
}

fn push_unique(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|existing| existing == id) {
        ids.push(id.to_owned());
    }
}

/// Sets `document[section][name]`, or removes it for `null`.
fn set_member(document: &mut Value, section: &str, name: &str, value: Value) {
    let Some(map) = document.as_object_mut() else {
        return;
    };
    if value.is_null() {
        if let Some(Value::Object(members)) = map.get_mut(section) {
            members.remove(name);
        }
        return;
    }
    let members = map.entry(section).or_insert_with(|| json!({}));
    if let Some(members) = members.as_object_mut() {
        members.insert(name.to_owned(), value);
    }
}

impl MapContext {
    /// Applies a style change and fetches loaded vector tiles again when it needs that; the old
    /// content stays on screen until the new tiles arrive.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        let change = apply(&mut self.style)?;
        if !change.removed_layers.is_empty() {
            crate::vector::content::purge_layers(&mut self.world, &change.removed_layers);
        }
        if change.redraw_tiles {
            tile_retry::refresh(&mut self.world, RequestKind::Vector);
        }
        Ok(change)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

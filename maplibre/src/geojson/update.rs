//! Replacing or editing the data of a GeoJSON source after the style was loaded.
//!
//! Every change is validated before it is applied and bumps the source's generation, which makes
//! workers index the new data and lets the request system replace loaded tiles.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

use super::index::{GeoJsonError, GeoJsonIndex};
use crate::{
    context::MapContext,
    io::tile_retry::{self, RequestKind},
    style::{
        source::{fresh_generation, GeoJsonData, GeoJsonSource, PromoteId, Source, GEOJSON_LAYER},
        Style,
    },
};

/// Why a source's data could not be changed. The source keeps its previous data.
#[derive(Debug, Error)]
pub enum SourceUpdateError {
    /// The style declares no source with this name.
    #[error("style has no source `{source_name}`")]
    UnknownSource {
        /// The requested name.
        source_name: String,
    },
    /// The source is not a GeoJSON source.
    #[error("source `{source_name}` is not a GeoJSON source")]
    NotGeoJson {
        /// The requested name.
        source_name: String,
    },
    /// A feature diff needs the document itself, which a URL source does not hold.
    #[error(
        "source `{source_name}` loads its data from a URL, so it cannot be updated by feature"
    )]
    NotInline {
        /// The requested name.
        source_name: String,
    },
    /// A feature to add carries no id to identify it by.
    #[error("a feature added to `{source_name}` has no id")]
    MissingId {
        /// The source being changed.
        source_name: String,
    },
    /// An update names a feature the source does not hold.
    #[error("source `{source_name}` has no feature with id {id}")]
    UnknownFeature {
        /// The source being changed.
        source_name: String,
        /// The id that matched nothing.
        id: Value,
    },
    /// The resulting document cannot be indexed.
    #[error("the new data of `{source_name}` is invalid")]
    Invalid {
        /// The source being changed.
        source_name: String,
        /// What is wrong with the document.
        #[source]
        source: GeoJsonError,
    },
}

/// A property to set on a feature, as GL JS `addOrUpdateProperties`.
#[derive(Debug, Clone, Deserialize)]
pub struct PropertyValue {
    /// Property name.
    pub key: String,
    /// New value.
    pub value: Value,
}

/// A change to one feature, as GL JS `GeoJSONFeatureDiff`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureUpdate {
    /// Id of the feature to change.
    pub id: Value,
    /// Replaces the geometry.
    #[serde(default)]
    pub new_geometry: Option<Value>,
    /// Properties to set.
    #[serde(default)]
    pub add_or_update_properties: Vec<PropertyValue>,
    /// Properties to drop.
    #[serde(default)]
    pub remove_properties: Vec<String>,
    /// Drops every property before the others are set.
    #[serde(default)]
    pub remove_all_properties: bool,
}

/// Feature-level changes to an inline source, as GL JS `GeoJSONSourceDiff`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoJsonDiff {
    /// Drops every feature first.
    #[serde(default)]
    pub remove_all: bool,
    /// Ids of features to drop.
    #[serde(default)]
    pub remove: Vec<Value>,
    /// Features to add; one with an existing id replaces it.
    #[serde(default)]
    pub add: Vec<Value>,
    /// Features to change.
    #[serde(default)]
    pub update: Vec<FeatureUpdate>,
}

impl Style {
    /// Replaces the data of a GeoJSON source, as GL JS `setData`.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: GeoJsonData,
    ) -> Result<(), SourceUpdateError> {
        let source = geojson_source_mut(self, source_name)?;
        if let GeoJsonData::Inline(document) = &data {
            GeoJsonIndex::from_value(document, source).map_err(|source| {
                SourceUpdateError::Invalid {
                    source_name: source_name.to_owned(),
                    source,
                }
            })?;
        }
        source.data = data;
        source.generation = fresh_generation();
        Ok(())
    }

    /// Adds, changes and removes features of an inline source, as GL JS `updateData`.
    pub fn update_geojson_data(
        &mut self,
        source_name: &str,
        diff: &GeoJsonDiff,
    ) -> Result<(), SourceUpdateError> {
        let source = geojson_source_mut(self, source_name)?;
        let GeoJsonData::Inline(document) = &source.data else {
            return Err(SourceUpdateError::NotInline {
                source_name: source_name.to_owned(),
            });
        };
        let promoted = promoted_property(source);
        let mut features = feature_list(document);
        apply(&mut features, diff, promoted.as_deref(), source_name)?;
        let updated = json!({"type": "FeatureCollection", "features": features});
        GeoJsonIndex::from_value(&updated, source).map_err(|source| {
            SourceUpdateError::Invalid {
                source_name: source_name.to_owned(),
                source,
            }
        })?;
        source.data = GeoJsonData::Inline(Arc::new(updated));
        source.generation = fresh_generation();
        Ok(())
    }
}

impl MapContext {
    /// Replaces a GeoJSON source's data and refreshes the tiles drawn from it. The old tiles stay
    /// on screen until the new ones arrive.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: GeoJsonData,
    ) -> Result<(), SourceUpdateError> {
        self.style.set_geojson_data(source_name, data)?;
        tile_retry::refresh(&mut self.world, RequestKind::Vector);
        Ok(())
    }

    /// Applies a feature diff to an inline GeoJSON source and refreshes its tiles.
    pub fn update_geojson_data(
        &mut self,
        source_name: &str,
        diff: &GeoJsonDiff,
    ) -> Result<(), SourceUpdateError> {
        self.style.update_geojson_data(source_name, diff)?;
        tile_retry::refresh(&mut self.world, RequestKind::Vector);
        Ok(())
    }
}

fn geojson_source_mut<'a>(
    style: &'a mut Style,
    name: &str,
) -> Result<&'a mut GeoJsonSource, SourceUpdateError> {
    match style.sources.get_mut(name) {
        Some(Source::GeoJson(source)) => Ok(source),
        Some(_) => Err(SourceUpdateError::NotGeoJson {
            source_name: name.to_owned(),
        }),
        None => Err(SourceUpdateError::UnknownSource {
            source_name: name.to_owned(),
        }),
    }
}

pub(super) fn promoted_property(source: &GeoJsonSource) -> Option<String> {
    match source.promote_id.as_ref()? {
        PromoteId::Property(name) => Some(name.clone()),
        PromoteId::PerLayer(map) => map.get(GEOJSON_LAYER).cloned(),
    }
}

/// The features of a document, whichever GeoJSON root it has.
pub(super) fn feature_list(document: &Value) -> Vec<Value> {
    match document.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => document
            .get("features")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        Some("Feature") => vec![document.clone()],
        Some(_) => vec![json!({"type": "Feature", "properties": {}, "geometry": document})],
        None => Vec::new(),
    }
}

/// The value a feature is addressed by: the promoted property, else its `id`.
pub(super) fn feature_id(feature: &Value, promoted: Option<&str>) -> Option<Value> {
    promoted
        .and_then(|name| feature.get("properties")?.get(name))
        .filter(|value| !value.is_null())
        .or_else(|| feature.get("id"))
        .cloned()
}

fn apply(
    features: &mut Vec<Value>,
    diff: &GeoJsonDiff,
    promoted: Option<&str>,
    source_name: &str,
) -> Result<(), SourceUpdateError> {
    if diff.remove_all {
        features.clear();
    }
    features.retain(|feature| {
        feature_id(feature, promoted).is_none_or(|id| !diff.remove.contains(&id))
    });
    for added in &diff.add {
        let id = feature_id(added, promoted).ok_or_else(|| SourceUpdateError::MissingId {
            source_name: source_name.to_owned(),
        })?;
        match features
            .iter_mut()
            .find(|feature| feature_id(feature, promoted).as_ref() == Some(&id))
        {
            Some(existing) => *existing = added.clone(),
            None => features.push(added.clone()),
        }
    }
    for update in &diff.update {
        let feature = features
            .iter_mut()
            .find(|feature| feature_id(feature, promoted).as_ref() == Some(&update.id))
            .ok_or_else(|| SourceUpdateError::UnknownFeature {
                source_name: source_name.to_owned(),
                id: update.id.clone(),
            })?;
        if let Some(geometry) = &update.new_geometry {
            feature["geometry"] = geometry.clone();
        }
        if update.remove_all_properties {
            feature["properties"] = json!({});
        }
        if !feature["properties"].is_object() {
            feature["properties"] = json!({});
        }
        if let Some(properties) = feature["properties"].as_object_mut() {
            for name in &update.remove_properties {
                properties.remove(name);
            }
            for property in &update.add_or_update_properties {
                properties.insert(property.key.clone(), property.value.clone());
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

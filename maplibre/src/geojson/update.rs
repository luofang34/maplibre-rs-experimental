//! Replacing or editing the data of a GeoJSON source after the style was loaded.
//!
//! Every change is validated before it is applied and bumps the source's generation, which makes
//! workers index the new data and lets the request system replace loaded tiles.

use std::{borrow::Cow, sync::Arc};

use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

use super::index::{GeoJsonError, GeoJsonIndex};
use crate::{
    context::MapContext,
    style::{
        source::{fresh_generation, GeoJsonData, GeoJsonSource, PromoteId, Source, GEOJSON_LAYER},
        Style,
    },
};

pub(crate) mod dynamic_points;

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

impl GeoJsonDiff {
    /// Parses a GL JS `GeoJSONSourceDiff` directly from a [`Value`].
    pub fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        use crate::util::json::extract::{
            field_err, json_bool_or, json_req_string, missing_field_err,
        };
        let Some(map) = value.as_object() else {
            return Err(field_err("diff", "expected object"));
        };
        let opt_values = |key: &str| -> Result<Vec<Value>, serde_json::Error> {
            match map.get(key) {
                None => Ok(Vec::new()),
                Some(v) => v
                    .as_array()
                    .cloned()
                    .ok_or_else(|| field_err(key, "expected array")),
            }
        };
        let update = match map.get("update") {
            None => Vec::new(),
            Some(v) => {
                let items = v
                    .as_array()
                    .ok_or_else(|| field_err("update", "expected array"))?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let obj = item
                        .as_object()
                        .ok_or_else(|| field_err("update", "expected object"))?;
                    let id = obj
                        .get("id")
                        .cloned()
                        .ok_or_else(|| missing_field_err("id"))?;
                    let new_geometry = obj.get("newGeometry").filter(|g| !g.is_null()).cloned();
                    let add_or_update_properties = match obj.get("addOrUpdateProperties") {
                        None => Vec::new(),
                        Some(props) => {
                            let arr = props.as_array().ok_or_else(|| {
                                field_err("addOrUpdateProperties", "expected array")
                            })?;
                            let mut pairs = Vec::with_capacity(arr.len());
                            for entry in arr {
                                let eobj = entry.as_object().ok_or_else(|| {
                                    field_err("addOrUpdateProperties", "expected object")
                                })?;
                                let key = json_req_string(eobj, "key")?;
                                let val = eobj
                                    .get("value")
                                    .cloned()
                                    .ok_or_else(|| missing_field_err("value"))?;
                                pairs.push(PropertyValue { key, value: val });
                            }
                            pairs
                        }
                    };
                    let remove_properties = crate::util::json::extract::json_string_vec_or_empty(
                        obj,
                        "removeProperties",
                    )?;
                    let remove_all_properties = json_bool_or(obj, "removeAllProperties", false)?;
                    out.push(FeatureUpdate {
                        id,
                        new_geometry,
                        add_or_update_properties,
                        remove_properties,
                        remove_all_properties,
                    });
                }
                out
            }
        };
        Ok(Self {
            remove_all: json_bool_or(map, "removeAll", false)?,
            remove: opt_values("remove")?,
            add: opt_values("add")?,
            update,
        })
    }
}

impl Style {
    /// Replaces the data of a GeoJSON source, as GL JS `setData`. Inside
    /// [`MapContext::mutate_style`] this needs no `redraw_tiles`: the map fetches again only the
    /// loaded tiles that features which changed lie in, as [`MapContext::set_geojson_data`] does.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: GeoJsonData,
    ) -> Result<(), SourceUpdateError> {
        let source = geojson_source_mut(self, source_name)?;
        let next_gen = fresh_generation();
        if let GeoJsonData::Inline(document) = &data {
            let index = GeoJsonIndex::from_value(document, source).map_err(|source| {
                SourceUpdateError::Invalid {
                    source_name: source_name.to_owned(),
                    source,
                }
            })?;
            super::store::prime_geojson_index(next_gen, Arc::new(index));
        }
        source.data = data;
        source.generation = next_gen;
        self.bump_revision();
        Ok(())
    }

    /// Replaces the data of a GeoJSON source from raw UTF-8 JSON bytes or a URL string.
    pub fn set_geojson_bytes(
        &mut self,
        source_name: &str,
        bytes: &[u8],
        was_string: bool,
    ) -> Result<(), SourceUpdateError> {
        let data = parse_geojson_bytes_payload(source_name, bytes, was_string)?;
        self.set_geojson_data(source_name, data)
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
        let mut features = feature_list(document).into_owned();
        apply(&mut features, diff, promoted.as_deref(), source_name)?;
        let updated = json!({"type": "FeatureCollection", "features": features});
        let index = GeoJsonIndex::from_value(&updated, source).map_err(|source| {
            SourceUpdateError::Invalid {
                source_name: source_name.to_owned(),
                source,
            }
        })?;
        let next_gen = fresh_generation();
        super::store::prime_geojson_index(next_gen, Arc::new(index));
        source.data = GeoJsonData::Inline(Arc::new(updated));
        source.generation = next_gen;
        self.bump_revision();
        Ok(())
    }
}

impl MapContext {
    /// Replaces a GeoJSON source's data and refreshes the loaded tiles it changes. The old tiles
    /// stay on screen until the new ones arrive. For inline data without clusters those are the
    /// tiles the added, changed and removed features lie in; otherwise every loaded tile.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: GeoJsonData,
    ) -> Result<(), SourceUpdateError> {
        if let GeoJsonData::Inline(ref doc) = data {
            if dynamic_points::try_apply_stationary_inline(
                &mut self.style,
                &mut self.world,
                source_name,
                doc,
            ) {
                return Ok(());
            }
        }
        dynamic_points::flush_deferred_geojson_bytes(&mut self.style, &mut self.world);
        let before = GeoJsonVersions::of(&self.style);
        self.style.set_geojson_data(source_name, data)?;
        dynamic_points::seed_from_inline(&self.style, &mut self.world, source_name);
        before.refresh_changed(&self.style, &mut self.world);
        Ok(())
    }

    /// Replaces a GeoJSON source's data from UTF-8 JSON `bytes` (or a URL string when `was_string`
    /// is true), taking the zero-DOM stationary-point fast path whenever eligible.
    pub fn set_geojson_bytes(
        &mut self,
        source_name: &str,
        bytes: &[u8],
        was_string: bool,
    ) -> Result<(), SourceUpdateError> {
        if !was_string || trim_ascii_start(bytes).starts_with(b"{") {
            if dynamic_points::try_apply_stationary_bytes(
                &self.style,
                &mut self.world,
                source_name,
                bytes,
            ) {
                return Ok(());
            }
        }
        let data = parse_geojson_bytes_payload(source_name, bytes, was_string)?;
        self.set_geojson_data(source_name, data)
    }

    /// Applies a feature diff to an inline GeoJSON source and refreshes the loaded tiles it
    /// changes, as [`Self::set_geojson_data`] does.
    pub fn update_geojson_data(
        &mut self,
        source_name: &str,
        diff: &GeoJsonDiff,
    ) -> Result<(), SourceUpdateError> {
        dynamic_points::flush_deferred_geojson_bytes(&mut self.style, &mut self.world);
        let before = GeoJsonVersions::of(&self.style);
        self.style.update_geojson_data(source_name, diff)?;
        dynamic_points::seed_from_inline(&self.style, &mut self.world, source_name);
        before.refresh_changed(&self.style, &mut self.world);
        Ok(())
    }
}

fn trim_ascii_start(bytes: &[u8]) -> &[u8] {
    let idx = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[idx..]
}

fn parse_geojson_bytes_payload(
    source_name: &str,
    bytes: &[u8],
    was_string: bool,
) -> Result<GeoJsonData, SourceUpdateError> {
    if was_string {
        let trimmed = trim_ascii_start(bytes);
        if !trimmed.starts_with(b"{") && !trimmed.starts_with(b"[") {
            let url = std::str::from_utf8(bytes).map_err(|_| {
                let err = serde_json::from_slice::<Value>(b"\xff")
                    .err()
                    .unwrap_or_else(|| serde::de::Error::custom("invalid UTF-8"));
                SourceUpdateError::Invalid {
                    source_name: source_name.to_owned(),
                    source: GeoJsonError::Json(err),
                }
            })?;
            return Ok(GeoJsonData::Url(url.to_owned()));
        }
    }
    let value =
        serde_json::from_slice::<Value>(bytes).map_err(|err| SourceUpdateError::Invalid {
            source_name: source_name.to_owned(),
            source: GeoJsonError::Json(err),
        })?;
    Ok(GeoJsonData::Inline(Arc::new(value)))
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
pub(super) fn feature_list(document: &Value) -> Cow<'_, [Value]> {
    match document.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => document
            .get("features")
            .and_then(Value::as_array)
            .map_or(Cow::Owned(Vec::new()), |features| {
                Cow::Borrowed(features.as_slice())
            }),
        Some("Feature") => Cow::Owned(vec![document.clone()]),
        Some(_) => Cow::Owned(vec![
            json!({"type": "Feature", "properties": {}, "geometry": document}),
        ]),
        None => Cow::Owned(Vec::new()),
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

mod refresh;
pub(crate) use refresh::GeoJsonVersions;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

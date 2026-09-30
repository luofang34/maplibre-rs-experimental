//! Reading the features a GeoJSON source holds, as GL JS `querySourceFeatures`.
//!
//! The answer comes from the source's document, not from loaded tiles, so it does not depend on
//! what is in view. Only a source declared with inline data (or given data by `setData`) can be
//! read here; a URL source's document is held by the workers that fetch it.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use super::update::{feature_id, feature_list, promoted_property};
use crate::{
    context::MapContext,
    style::{
        expression::Value as StyleValue,
        filter::{properties_from_json, FeatureContext, Filter, FilterError, GeometryType},
        source::{GeoJsonData, Source, GEOJSON_LAYER},
        Style,
    },
};

/// Narrows a source query, as the parameters of GL JS `querySourceFeatures`.
#[derive(Clone, Debug, Default)]
pub struct SourceQueryOptions {
    /// Source layer to read; a GeoJSON source has the single layer `_geojson`.
    pub source_layer: Option<String>,
    /// A layer filter, in the legacy or the expression syntax.
    pub filter: Option<Value>,
}

/// One feature of a source, as the document declares it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SourceFeature {
    /// Style source id.
    pub source: String,
    /// The `_geojson` layer name.
    pub source_layer: String,
    /// The feature id, or its position in the document with `generateId`.
    pub id: Option<Value>,
    /// The feature's own properties.
    pub properties: BTreeMap<String, Value>,
    /// The geometry type name, such as `Point` or `MultiPolygon`.
    pub geometry_type: String,
    /// The unclipped GeoJSON geometry.
    pub geometry: Value,
}

/// Why a source could not be read.
#[derive(Debug, Error, PartialEq)]
pub enum SourceQueryError {
    /// The style declares no source with this name.
    #[error("style has no source `{source_name}`")]
    UnknownSource {
        /// The requested name.
        source_name: String,
    },
    /// Only GeoJSON sources can be read feature by feature here.
    #[error("source `{source_name}` is not a GeoJSON source")]
    NotGeoJson {
        /// The requested name.
        source_name: String,
    },
    /// The data is a URL, so the document is not available on this thread.
    #[error("source `{source_name}` loads its data from a URL, so its features are not available")]
    NotLoaded {
        /// The requested name.
        source_name: String,
    },
    /// The filter is not valid.
    #[error("query filter is invalid")]
    InvalidFilter(#[source] FilterError),
}

impl Style {
    /// The features of an inline GeoJSON source in document order, restricted by a source layer
    /// and a filter. A source layer other than `_geojson` matches nothing.
    pub fn query_source_features(
        &self,
        source_name: &str,
        options: &SourceQueryOptions,
    ) -> Result<Vec<SourceFeature>, SourceQueryError> {
        let source = match self.sources.get(source_name) {
            Some(Source::GeoJson(source)) => source,
            Some(_) => {
                return Err(SourceQueryError::NotGeoJson {
                    source_name: source_name.to_owned(),
                })
            }
            None => {
                return Err(SourceQueryError::UnknownSource {
                    source_name: source_name.to_owned(),
                })
            }
        };
        let GeoJsonData::Inline(document) = &source.data else {
            return Err(SourceQueryError::NotLoaded {
                source_name: source_name.to_owned(),
            });
        };
        let filter = options
            .filter
            .as_ref()
            .map(Filter::parse)
            .transpose()
            .map_err(SourceQueryError::InvalidFilter)?;
        if options
            .source_layer
            .as_deref()
            .is_some_and(|layer| layer != GEOJSON_LAYER)
        {
            return Ok(Vec::new());
        }
        let promoted = promoted_property(source);
        let mut found = Vec::new();
        for (position, feature) in feature_list(document).iter().enumerate() {
            let Some(geometry) = feature.get("geometry").filter(|value| !value.is_null()) else {
                continue;
            };
            let geometry_type = geometry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = feature_id(feature, promoted.as_deref())
                .or_else(|| source.generate_id.then(|| Value::from(position as u64)));
            if let Some(filter) = &filter {
                let properties = properties_from_json(feature.get("properties"));
                let passes = filter.evaluate(&FeatureContext {
                    properties: &properties,
                    geometry_type: GeometryType::from_geojson(geometry_type),
                    id: id.as_ref().map(StyleValue::from_json),
                    zoom: 0.0,
                });
                if !passes {
                    continue;
                }
            }
            found.push(SourceFeature {
                source: source_name.to_owned(),
                source_layer: GEOJSON_LAYER.to_owned(),
                id,
                properties: feature
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
                geometry_type: geometry_type.to_owned(),
                geometry: geometry.clone(),
            });
        }
        Ok(found)
    }
}

impl MapContext {
    /// The features of an inline GeoJSON source; see [`Style::query_source_features`].
    pub fn query_source_features(
        &self,
        source_name: &str,
        options: &SourceQueryOptions,
    ) -> Result<Vec<SourceFeature>, SourceQueryError> {
        self.style.query_source_features(source_name, options)
    }

    /// Placed symbols under a point or box, topmost first; see
    /// [`crate::sdf::query::query_rendered_symbols_in`].
    pub fn query_rendered_symbols(
        &self,
        geometry: crate::sdf::query::QueryGeometry,
        options: &crate::sdf::query::QueryOptions,
    ) -> Result<Vec<crate::sdf::query::RenderedSymbol>, crate::sdf::query::QueryError> {
        crate::sdf::query::query_rendered_symbols_in(&self.world, &self.style, geometry, options)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

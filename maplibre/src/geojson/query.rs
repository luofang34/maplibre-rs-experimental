//! Reading the features a GeoJSON source holds, as GL JS `querySourceFeatures`.
//!
//! The answer comes from the source's document, not from loaded tiles, so it does not depend on
//! what is in view. Only a source declared with inline data (or given data by `setData`) can be
//! read here; a URL source's document is held by the workers that fetch it.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use super::update::{feature_list, promoted_property};
use crate::{
    context::MapContext,
    style::{
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
    /// The feature id (numeric or string, respecting `promoteId` and `generateId`).
    pub id: Option<crate::query::QueriedFeatureId>,
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
        self.query_source_features_with_override(source_name, None, options)
    }

    pub(crate) fn query_source_features_with_override(
        &self,
        source_name: &str,
        inline_override: Option<&Value>,
        options: &SourceQueryOptions,
    ) -> Result<Vec<SourceFeature>, SourceQueryError> {
        let source = resolve_geojson_source(&self.sources, source_name)?;
        let document = resolve_source_document(source_name, source, inline_override)?;
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
        let source_filter = match parse_source_filter(source_name, source) {
            Some(parsed) => parsed,
            None => return Ok(Vec::new()),
        };
        let promoted = promoted_property(source);
        Ok(feature_list(document)
            .iter()
            .enumerate()
            .filter_map(|(position, feature)| {
                match_source_feature(
                    source_name,
                    feature,
                    position,
                    promoted.as_deref(),
                    source.generate_id,
                    source_filter.as_ref(),
                    filter.as_ref(),
                )
            })
            .collect())
    }
}

fn resolve_geojson_source<'a>(
    sources: &'a rustc_hash::FxHashMap<String, Source>,
    source_name: &str,
) -> Result<&'a crate::style::source::GeoJsonSource, SourceQueryError> {
    match sources.get(source_name) {
        Some(Source::GeoJson(source)) => Ok(source),
        Some(_) => Err(SourceQueryError::NotGeoJson {
            source_name: source_name.to_owned(),
        }),
        None => Err(SourceQueryError::UnknownSource {
            source_name: source_name.to_owned(),
        }),
    }
}

fn resolve_source_document<'a>(
    source_name: &str,
    source: &'a crate::style::source::GeoJsonSource,
    inline_override: Option<&'a Value>,
) -> Result<&'a Value, SourceQueryError> {
    match (inline_override, &source.data) {
        (Some(doc), _) => Ok(doc),
        (None, GeoJsonData::Inline(document)) => Ok(document),
        (None, GeoJsonData::Url(_)) => Err(SourceQueryError::NotLoaded {
            source_name: source_name.to_owned(),
        }),
    }
}

fn parse_source_filter(
    source_name: &str,
    source: &crate::style::source::GeoJsonSource,
) -> Option<Option<Filter>> {
    match source.filter.as_ref() {
        Some(raw) => match Filter::parse(raw) {
            Ok(parsed) => Some(Some(parsed)),
            Err(error) => {
                tracing::warn!(
                    source = source_name,
                    %error,
                    "invalid GeoJSON source filter; query returns no features"
                );
                None
            }
        },
        None => Some(None),
    }
}

fn match_source_feature(
    source_name: &str,
    feature: &Value,
    position: usize,
    promoted: Option<&str>,
    generate_id: bool,
    source_filter: Option<&Filter>,
    query_filter: Option<&Filter>,
) -> Option<SourceFeature> {
    let geometry = feature.get("geometry").filter(|value| !value.is_null())?;
    let geometry_type = geometry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = if let Some(prop) = promoted {
        feature
            .get("properties")
            .and_then(|props| props.get(prop))
            .and_then(crate::query::QueriedFeatureId::from_json)
    } else if generate_id {
        Some(crate::query::QueriedFeatureId::Number(position as u64))
    } else {
        feature
            .get("id")
            .and_then(crate::query::QueriedFeatureId::from_json)
    };
    if source_filter.is_some() || query_filter.is_some() {
        let properties = properties_from_json(feature.get("properties"));
        let ctx = FeatureContext {
            properties: &properties,
            geometry_type: GeometryType::from_geojson(geometry_type),
            id: id
                .as_ref()
                .map(crate::query::QueriedFeatureId::to_expr_value),
            zoom: 0.0,
        };
        if source_filter.is_some_and(|sf| !sf.evaluate(&ctx)) {
            return None;
        }
        if query_filter.is_some_and(|qf| !qf.evaluate(&ctx)) {
            return None;
        }
    }
    Some(SourceFeature {
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
    })
}

impl MapContext {
    /// The features of an inline GeoJSON source; see [`Style::query_source_features`].
    pub fn query_source_features(
        &self,
        source_name: &str,
        options: &SourceQueryOptions,
    ) -> Result<Vec<SourceFeature>, SourceQueryError> {
        let deferred =
            super::update::dynamic_points::deferred_inline_value(&self.world, source_name);
        self.style
            .query_source_features_with_override(source_name, deferred.as_ref(), options)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

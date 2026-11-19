//! Feature property lookup helpers against resident [`TileIndex`] storage.

use std::sync::Arc;

use super::{IndexedGeometry, TileIndex};
use crate::style::expression::FeatureProperties;

impl TileIndex {
    /// Looks up the full property map of the feature at `feature_index` within `source_layer`.
    pub fn feature_properties(
        &self,
        source_layer: &str,
        feature_index: u32,
    ) -> Option<Arc<FeatureProperties>> {
        let find_in = |items: &[IndexedGeometry<f64>]| -> Option<Arc<FeatureProperties>> {
            items
                .iter()
                .find(|item| {
                    item.feature_index == feature_index
                        && item.source_layer.as_ref() == source_layer
                })
                .map(|item| Arc::clone(&item.properties))
        };
        match self {
            Self::Spatial { tree } => tree
                .iter()
                .find(|item| {
                    item.feature_index == feature_index
                        && item.source_layer.as_ref() == source_layer
                })
                .map(|item| Arc::clone(&item.properties)),
            Self::Linear { list } => find_in(list),
            Self::LazyMvt(lazy) => find_in(lazy.geometries()),
        }
    }
}

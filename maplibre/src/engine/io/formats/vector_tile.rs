//! CPU decoding, slicing, rescaling, and property extraction for MVT and MLT vector tiles.

pub(crate) mod decode;
#[cfg(feature = "mlt")]
pub mod mlt;
#[cfg(feature = "mlt")]
pub(crate) mod mlt_index;
pub(crate) mod mvt_slice;
pub(crate) mod property;
pub mod rescale;

use geozero::mvt::tile;

pub(crate) use crate::io::vector_tile::{
    decode::*, mvt_slice::fill_feature_properties, property::property_value,
};
#[cfg(feature = "mlt")]
pub(crate) use crate::io::vector_tile::{mlt_index::visit_mlt_features, mvt_slice::looks_like_mvt};
use crate::{coords::EXTENT, style::expression::FeatureProperties};

/// Scale from a layer's declared coordinate extent to the 4096 grid the shaders expect.
pub(crate) fn extent_scale(layer: &tile::Layer) -> f64 {
    match layer.extent {
        Some(extent) if extent > 0 => EXTENT / f64::from(extent),
        _ => 1.0,
    }
}

/// Pre-decoded MVT protobuf or validated zero-copy MLT columnar byte slice shared across base and
/// symbol passes of a tile worker.
pub(crate) enum DecodedVectorTile {
    Mvt(geozero::mvt::Tile),
    #[cfg(feature = "mlt")]
    Mlt(std::sync::Arc<[u8]>),
}

/// Reads an MVT feature's tags into typed filter values through the layer's key and value
/// tables.
pub(crate) fn feature_properties(
    layer: &tile::Layer,
    feature: &tile::Feature,
) -> FeatureProperties {
    let mut properties = FeatureProperties::default();
    fill_feature_properties(layer, feature, &mut properties);
    properties
}

//! The index a GeoJSON tile sends back so rendered queries find its fills, lines and
//! extrusions as they find a vector tile's.

use geozero::GeozeroDatasource;

use super::{ProcessGeoJsonError, ProjectingTessellator};
use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::{
        apc::Context,
        geometry_index::{IndexProcessor, IndexedGeometry, TileIndex},
    },
    style::source::GEOJSON_LAYER,
    vector::transferables::{LayerIndexed, VectorTransferables},
};

/// The parts of the geometries within the tile and its buffer, as GL JS's GeoJSON tiles hold
/// them, that reach the tile itself.
fn within_tile(
    geometries: Vec<IndexedGeometry<f64>>,
    buffer: Option<u32>,
) -> Vec<IndexedGeometry<f64>> {
    let extent = EXTENT;
    // The buffer is in pixels of a 512-pixel tile, as GL JS's geojson-vt takes it.
    let margin = f64::from(buffer.unwrap_or(128)) * extent / 512.0;
    let tile = geo_types::Rect::new(
        geo_types::Coord {
            x: -margin,
            y: -margin,
        },
        geo_types::Coord {
            x: extent + margin,
            y: extent + margin,
        },
    );
    geometries
        .into_iter()
        .flat_map(|geometry| geometry.clipped_to(tile))
        .filter(|part| {
            // GL JS `FeatureIndex.insert` leaves out a part that lies in the buffer alone: the
            // neighbour the buffer copies it from finds it.
            let (lower, upper) = (part.bounds.lower(), part.bounds.upper());
            lower.x() < extent && lower.y() < extent && upper.x() >= 0.0 && upper.y() >= 0.0
        })
        .collect()
}

/// Indexes the tile's features once for all the source's layers, so rendered queries find its
/// fills, lines and extrusions as they find a vector tile's.
pub(super) fn send<T: VectorTransferables, C: Context>(
    geojson_value: &serde_json::Value,
    unfiltered: &str,
    (coords, source, buffer): (WorldTileCoords, &str, Option<u32>),
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    let ids = super::index::feature_ids(geojson_value);
    let mut index = IndexProcessor::new();
    index.begin_layer(GEOJSON_LAYER, ids);
    let mut projecting = ProjectingTessellator::new(coords, index);
    // A tile whose features cannot be indexed still renders; queries then miss it.
    if let Err(error) = geozero::geojson::GeoJson(unfiltered).process(&mut projecting) {
        tracing::warn!(%coords, ?error, "skipping query index for GeoJSON tile");
        return Ok(());
    }
    context
        .send_back(T::LayerIndexed::build_from(
            coords,
            Some(source.to_owned()),
            TileIndex::Linear {
                list: within_tile(
                    {
                        let mut index = projecting.into_inner();
                        index.commit_bare_geometry();
                        index.get_geometries()
                    },
                    buffer,
                ),
            },
        ))
        .map_err(ProcessGeoJsonError::SendError)
}

#[cfg(test)]
mod tests;

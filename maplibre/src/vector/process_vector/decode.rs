//! Container-level tile decoding (`MVT` with optional zero-padding and `MLT` fallback).

use geozero::mvt::Message;

use super::mvt_slice;

/// Decodes a tile, tolerating the zero padding some tile servers append: a zero tag is not a
/// field, so a strict decoder rejects a tile that GL JS reads.
pub(crate) fn decode_tile(
    data: &[u8],
) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error + Send + Sync>> {
    // A payload that is not a protobuf tile with layers is tried as a MapLibre Tile.
    #[cfg(feature = "mlt")]
    match decode_mvt(data) {
        Ok(tile) if !tile.layers.is_empty() => Ok(tile),
        Ok(tile) => Ok(crate::vector::mlt::decode(data).unwrap_or(tile)),
        Err(error) => crate::vector::mlt::decode(data).map_err(|_| error),
    }
    #[cfg(not(feature = "mlt"))]
    decode_mvt(data)
}

fn decode_mvt(data: &[u8]) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error + Send + Sync>> {
    geozero::mvt::Tile::decode(data).or_else(|error| {
        let end = data
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |i| i + 1);
        if end == data.len() {
            return Err(error.into());
        }
        geozero::mvt::Tile::decode(&data[..end]).map_err(|_| error.into())
    })
}

/// Decodes only the layers accepted by `layer_keys`, pruning MVT property tables to referenced
/// keys where `layer_keys` returns `Some(Some(keys))` and optionally clipping features into a
/// descendant tile's extent during decoding. Falls back cleanly for non-MVT or malformed tiles.
pub(crate) fn decode_tile_for_layers_with_keys_rescaled<'a>(
    data: &[u8],
    rescale: Option<(
        crate::coords::WorldTileCoords,
        crate::coords::WorldTileCoords,
    )>,
    mut layer_keys: impl FnMut(&str) -> Option<Option<&'a rustc_hash::FxHashSet<String>>>,
) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(tile) =
        mvt_slice::try_decode_mvt_filtered_with_keys_rescaled(data, rescale, &mut layer_keys)
    {
        return Ok(tile);
    }
    let mut tile = decode_tile(data)?;
    if let Some((from, to)) = rescale {
        crate::vector::request_system::rescale::into_descendant(&mut tile, from, to);
    }
    Ok(tile)
}

/// Decodes raw MVT/MLT bytes into tile-local [`crate::io::geometry_index::IndexedGeometry`]
/// entries, optionally bringing ancestor MVT tiles into a descendant tile's extent on demand.
pub(crate) fn index_mvt_bytes_rescaled(
    mvt_bytes: &[u8],
    rescale: Option<(
        crate::coords::WorldTileCoords,
        crate::coords::WorldTileCoords,
    )>,
    promote_id: Option<&crate::style::source::PromoteId>,
    feature_state: Option<&std::sync::Arc<crate::style::feature_state::SourceFeatureState>>,
) -> Vec<crate::io::geometry_index::IndexedGeometry<f64>> {
    #[cfg(feature = "mlt")]
    if !mvt_slice::looks_like_mvt(mvt_bytes) && crate::vector::mlt::is_valid_mlt(mvt_bytes) {
        return crate::vector::mlt::index_mlt_bytes(mvt_bytes, promote_id, feature_state);
    }
    let Ok(mut tile) = decode_tile(mvt_bytes) else {
        return Vec::new();
    };
    if let Some((from, to)) = rescale {
        crate::vector::request_system::rescale::into_descendant(&mut tile, from, to);
    }
    mvt_slice::index_decoded_mvt_tile(&tile, promote_id, feature_state)
}

/// Decodes only the layers of `data` whose names satisfy `keep`, falling back to [`decode_tile`]
/// when `data` uses non-MVT framing (such as MapLibre Tiles) or needs full wire error reporting.
pub(crate) fn decode_tile_for_layers(
    data: &[u8],
    keep: impl Fn(&str) -> bool,
) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(tile) = mvt_slice::try_decode_mvt_filtered(data, &keep) {
        return Ok(tile);
    }
    let mut tile = decode_tile(data)?;
    tile.layers.retain(|mvt| keep(mvt.name.as_str()));
    Ok(tile)
}

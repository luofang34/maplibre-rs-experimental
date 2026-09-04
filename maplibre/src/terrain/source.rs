//! Resolves the `raster-dem` source a style's terrain refers to.

use crate::{
    io::source_type::{RasterSource, SourceType},
    style::{
        source::{Source, TileAddressingScheme},
        Style,
    },
};

/// Highest zoom the style spec allows for a `raster-dem` source without an explicit maximum.
const DEFAULT_MAX_ZOOM: u8 = 22;

/// Fetchable DEM source together with the properties the terrain pipeline needs.
#[derive(Clone, Debug)]
pub struct DemSource {
    /// Name of the source in the style.
    pub name: String,
    /// Tile URL template resolved into a fetchable source.
    pub source: SourceType,
    /// Edge length of one DEM tile in pixels.
    pub tile_size: u32,
    /// Channel factors and base shift decoding pixels to metres.
    pub unpack: [f64; 4],
    /// Lowest zoom with tiles.
    pub minzoom: u8,
    /// Highest zoom with tiles.
    pub maxzoom: u8,
    /// Elevation multiplier from the terrain property.
    pub exaggeration: f32,
}

/// Returns the DEM source behind the style's `terrain`, or `None` when terrain is off or the
/// named source has no tile URLs yet.
pub fn dem_source(style: &Style) -> Option<DemSource> {
    let terrain = style.terrain.as_ref()?;
    let Some(Source::RasterDem(dem)) = style.sources.get(&terrain.source) else {
        tracing::warn!(source = %terrain.source, "terrain names a source that is not raster-dem");
        return None;
    };
    let template = dem.tiles.as_ref()?.first()?;
    Some(DemSource {
        name: terrain.source.clone(),
        source: SourceType::Raster(RasterSource::from_template(
            template,
            TileAddressingScheme::XYZ,
        )),
        tile_size: dem.tile_size,
        unpack: dem.unpack_vector(),
        minzoom: dem.minzoom.unwrap_or(0),
        maxzoom: dem.maxzoom.unwrap_or(DEFAULT_MAX_ZOOM),
        exaggeration: terrain.exaggeration,
    })
}

#[cfg(test)]
mod tests;

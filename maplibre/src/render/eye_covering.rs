//! One tile covering for both eyes of a frame.
//!
//! Each eye of a stereo pair runs the whole schedule on its own view. Selecting the drawn
//! tiles per eye lets the two eyes disagree about a tile's level where the per-tile level
//! changes, which the viewer sees as one tile at two resolutions, and costs a covering per
//! eye. The first eye's selection is kept for the frame and the other eyes draw the same
//! tiles with their own matrices; the loose padding of the request systems covers the few
//! centimetres between the eyes.

use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel},
    render::{
        projection::{raster_source_regions, view_region_for_projection, ProjectionStateError},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
};

/// Which eye of a frame the schedule is running for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EyeInFrame {
    /// Position of the eye in the frame; the first eye selects the tiles.
    pub index: usize,
    /// The frame, so a selection is never reused across frames.
    pub frame: u64,
}

/// Tiles per raster source, as [`raster_source_regions`] selects them.
pub type RasterCoverings = Vec<(String, Vec<WorldTileCoords>)>;

/// The drawn tiles the first eye of a frame selected.
#[derive(Debug)]
pub struct SharedCovering {
    frame: u64,
    tiles: Option<(Vec<WorldTileCoords>, ZoomLevel)>,
    raster: RasterCoverings,
}

impl SharedCovering {
    /// The tiles the first eye selected, or `None` when it selected no region.
    #[cfg(test)]
    pub(crate) fn tiles(&self) -> Option<&[WorldTileCoords]> {
        self.tiles.as_ref().map(|(tiles, _)| tiles.as_slice())
    }

    fn region(&self) -> Option<ViewRegion> {
        self.tiles
            .as_ref()
            .map(|(tiles, level)| ViewRegion::from_tiles(tiles.clone(), *level, tiles.len()))
    }
}

/// The tiles the frame draws at `level`, with the raster tiles each raster source draws:
/// the first eye's selection when the schedule runs for a later eye of the same frame, and
/// otherwise this view's own.
pub(crate) fn drawn_covering(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    level: ZoomLevel,
) -> Result<(Option<ViewRegion>, RasterCoverings), ProjectionStateError> {
    let eye = world.resources.get::<EyeInFrame>().copied();
    if let Some(eye) = eye.filter(|eye| eye.index > 0) {
        if let Some(shared) = world
            .resources
            .get::<SharedCovering>()
            .filter(|shared| shared.frame == eye.frame)
        {
            return Ok((shared.region(), shared.raster.clone()));
        }
    }
    let region =
        view_region_for_projection(style, view_state, world, level, ViewStatePadding::Tight)?;
    let raster = raster_source_regions(style, view_state, world, ViewStatePadding::Tight)?;
    if let Some(eye) = eye.filter(|eye| eye.index == 0) {
        world.resources.insert(SharedCovering {
            frame: eye.frame,
            tiles: region
                .as_ref()
                .map(|region| (region.iter().collect(), region.zoom_level())),
            raster: raster.clone(),
        });
    }
    Ok((region, raster))
}

#[cfg(test)]
mod tests;

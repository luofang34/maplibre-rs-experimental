//! One tile covering for both eyes of a frame.
//!
//! Tile selection and content ingestion happen once per stereo frame. Each eye then draws
//! that selection with its own projection, so a tile cannot change level or content between
//! the left and right images.

use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel},
    projection::lod_history::LodHistory,
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

impl EyeInFrame {
    /// Whether this eye must reuse the content selected for an earlier eye of the frame.
    pub(crate) fn reuses_content(world: &World) -> bool {
        world
            .resources
            .get::<Self>()
            .is_some_and(|eye| eye.index > 0)
    }
}

/// Tiles per raster source, as [`raster_source_regions`] selects them.
pub type RasterCoverings = Vec<(String, Vec<WorldTileCoords>)>;

/// An immutable snapshot used by every request and draw in the stereo frame.
#[derive(Default)]
pub(crate) struct FrameLodHistory {
    pub(crate) view: LodHistory,
    pub(crate) raster: std::collections::HashMap<String, LodHistory>,
}

pub(crate) fn snapshot_lod_history(world: &mut World) {
    if EyeInFrame::reuses_content(world) {
        return;
    }
    let history = world
        .resources
        .get::<SharedCovering>()
        .map(|shared| FrameLodHistory {
            view: LodHistory::new(
                shared
                    .tiles
                    .as_ref()
                    .map_or(&[], |(tiles, _)| tiles.as_slice()),
            ),
            raster: shared
                .raster
                .iter()
                .map(|(name, tiles)| (name.clone(), LodHistory::new(tiles)))
                .collect(),
        })
        .unwrap_or_default();
    world.resources.insert(history);
}

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

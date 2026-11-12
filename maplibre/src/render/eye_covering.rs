//! One tile covering for both eyes of a frame.
//!
//! Tile selection and content ingestion happen once per stereo frame. Each eye then draws
//! that selection with its own projection, so a tile cannot change level or content between
//! the left and right images.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel},
    projection::lod_history::LodHistory,
    raster::RasterSourceId,
    render::{
        projection::{raster_source_regions, view_region_for_projection, ProjectionStateError},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
};

/// Identifier of a registered viewport slot in a multi-view frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewSlotId(u32);

impl ViewSlotId {
    /// Wraps a raw slot index (`0..64` maps to bit positions in `ViewSlotMask`).
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw zero-based slot index.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Which [`ViewSlotId`] is currently executing its render stages, or `None` on the default head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CurrentViewSlot(pub Option<ViewSlotId>);

/// The view whose terrain drapes the slot being drawn shows: its own, or for the second eye
/// of a stereo pair the first eye's, whose terrain it replays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DrapeView(pub Option<ViewSlotId>);

/// Union of tiles and terrain drape coordinates retained across all active and skipped view slots.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlotResidencyFloor {
    pub(crate) drawn_count: usize,
    pub(crate) retained_tiles: FxHashSet<WorldTileCoords>,
    pub(crate) retained_drapes: FxHashSet<WorldTileCoords>,
}

/// Unioned tile requests computed across all active slots (plus skipped slots' in-flight sets)
/// before the single `Extract` stage of a multi-view frame.
#[derive(Clone, Debug, Default)]
pub(crate) struct MultiViewExtractCovering {
    pub(crate) active: bool,
    pub(crate) vector_tiles: Vec<WorldTileCoords>,
    pub(crate) max_view_level: u8,
    pub(crate) raster_regions: RasterCoverings,
    pub(crate) terrain_tiles: Vec<WorldTileCoords>,
    pub(crate) skipped_wanted_vector: FxHashSet<WorldTileCoords>,
    pub(crate) skipped_wanted_raster: FxHashSet<WorldTileCoords>,
    pub(crate) skipped_wanted_dem: FxHashSet<WorldTileCoords>,
}

/// Per-frame `Loose` request coverings recorded during single-view `Request` systems so
/// `retention_system` in `Cleanup` reuses them without re-walking the globe quadtree.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameLooseCovering {
    pub(crate) host_frame: u64,
    pub(crate) view_tiles: Option<Vec<WorldTileCoords>>,
    pub(crate) raster_tiles: Option<Vec<WorldTileCoords>>,
}

/// Total number of eye passes executing in the current [`XrFrame`](crate::render::xr::XrFrame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EyePassCount(pub usize);

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
pub type RasterCoverings = Vec<(RasterSourceId, Vec<WorldTileCoords>)>;

/// The tiles the previous frame drew, per covering, as one immutable snapshot every request
/// and draw of this frame selects against. Each covering keeps its own history: the terrain
/// surface, the view and each raster source split the quadtree at different levels, so one
/// covering's leaves say nothing about where another split.
#[derive(Clone, Default)]
pub(crate) struct FrameLodHistory {
    pub(crate) view: LodHistory,
    pub(crate) raster: FxHashMap<RasterSourceId, LodHistory>,
    pub(crate) terrain: LodHistory,
}

/// Takes the snapshot for the frame about to select tiles. Every camera gets it, not only a
/// host's eye: a map camera that moves a little each frame would otherwise switch a tile
/// sitting on a level threshold between its fine and coarse version frame after frame.
pub(crate) fn snapshot_lod_history(world: &mut World) {
    if EyeInFrame::reuses_content(world) {
        return;
    }
    let shared = world.resources.get::<SharedCovering>();
    let history = FrameLodHistory {
        view: LodHistory::new(
            shared
                .and_then(|shared| shared.tiles.as_ref())
                .map_or(&[], |(tiles, _)| tiles.as_slice()),
        ),
        raster: shared
            .map(|shared| {
                shared
                    .raster
                    .iter()
                    .map(|(name, tiles)| (name.clone(), LodHistory::new(tiles)))
                    .collect()
            })
            .unwrap_or_default(),
        terrain: LodHistory::new(crate::terrain::drawn_surface_tiles(world)),
    };
    world.resources.insert(history);
}

/// The drawn tiles a frame selected: the first eye's, which later eyes of the frame reuse, or
/// the map camera's own.
#[derive(Clone, Debug)]
pub struct SharedCovering {
    /// The stereo frame whose eyes share the selection; none for a map camera's frame.
    frame: Option<u64>,
    tiles: Option<(Vec<WorldTileCoords>, ZoomLevel)>,
    raster: RasterCoverings,
}

impl SharedCovering {
    /// Binds a cloned covering snapshot to `frame` for `CoveringPolicy::StereoReuse`.
    #[cfg(feature = "headless")]
    pub(crate) fn with_frame(&self, frame: u64) -> Self {
        Self {
            frame: Some(frame),
            tiles: self.tiles.clone(),
            raster: self.raster.clone(),
        }
    }

    /// The tiles the first eye selected, or `None` when it selected no region.
    pub(crate) fn tiles(&self) -> Option<&[WorldTileCoords]> {
        self.tiles.as_ref().map(|(tiles, _)| tiles.as_slice())
    }

    fn region(&self) -> Option<ViewRegion> {
        self.tiles
            .as_ref()
            .map(|(tiles, level)| ViewRegion::from_tiles(tiles.clone(), *level, tiles.len()))
    }
}

/// The tiles the frame's covering selected, each once, or `None` before a covering is selected
/// or when it selected no region.
pub(crate) fn drawn_tiles(world: &World) -> Option<&[WorldTileCoords]> {
    world.resources.get::<SharedCovering>()?.tiles()
}

/// The tiles the frame draws at `level`, with the raster tiles each raster source draws:
/// the first eye's selection when the schedule runs for a later eye of the same frame, and
/// otherwise this view's own, kept as the next frame's level history.
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
            .filter(|shared| shared.frame == Some(eye.frame))
        {
            return Ok((shared.region(), shared.raster.clone()));
        }
    }
    let region =
        view_region_for_projection(style, view_state, world, level, ViewStatePadding::Tight)?;
    let raster = raster_source_regions(style, view_state, world, ViewStatePadding::Tight)?;
    if eye.is_none_or(|eye| eye.index == 0) {
        world.resources.insert(SharedCovering {
            frame: eye.map(|eye| eye.frame),
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

//! One tile covering for both eyes of a frame.
//!
//! Tile selection and content ingestion happen once per stereo frame. Each eye then draws
//! that selection with its own projection, so a tile cannot change level or content between
//! the left and right images.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel},
    io::tile_sources::RasterSourceId,
    projection::lod_history::LodHistory,
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
    #[cfg_attr(not(feature = "raster"), allow(dead_code))]
    pub(crate) raster_regions: RasterCoverings,
    pub(crate) terrain_tiles: Vec<WorldTileCoords>,
    pub(crate) skipped_wanted_vector: FxHashSet<WorldTileCoords>,
    #[cfg_attr(not(feature = "raster"), allow(dead_code))]
    pub(crate) skipped_wanted_raster: FxHashSet<WorldTileCoords>,
    pub(crate) skipped_wanted_dem: FxHashSet<WorldTileCoords>,
}

pub(crate) type CachedViewTiles = (Option<Arc<[WorldTileCoords]>>, ViewStatePadding, ZoomLevel);

/// Per-frame request coverings recorded during single-view `Request` systems so
/// `retention_system` and `drawn_covering` reuse them without re-walking the globe quadtree.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameLooseCovering {
    pub(crate) host_frame: u64,
    pub(crate) view_tiles: Option<CachedViewTiles>,
    pub(crate) raster_regions: Option<(RasterCoverings, ViewStatePadding, (u64, u64, usize))>,
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
    host_frame: u64,
    style_rev: u64,
    level: ZoomLevel,
    view_proj_bits: [[u64; 4]; 4],
    terrain_key: (u64, u64, usize),
    tiles: Option<(Vec<WorldTileCoords>, ZoomLevel)>,
    raster: RasterCoverings,
}

impl SharedCovering {
    /// Binds a cloned covering snapshot to `frame` for `CoveringPolicy::StereoReuse`.
    #[cfg(feature = "headless")]
    pub(crate) fn with_frame(&self, frame: u64) -> Self {
        Self {
            frame: Some(frame),
            ..self.clone()
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

fn frame_view_proj_bits(world: &World, style: &Style, view_state: &ViewState) -> [[u64; 4]; 4] {
    let view_proj = world
        .resources
        .get::<crate::render::resolved_view::ResolvedFrameView>()
        .filter(|resolved| resolved.matches(style, view_state))
        .map(|resolved| resolved.view_projection)
        .unwrap_or_else(|| view_state.view_projection());
    (*AsRef::<[[f64; 4]; 4]>::as_ref(&view_proj.0)).map(|col| col.map(f64::to_bits))
}

/// Returns already-selected tight-padding tiles for this host frame when view projection and
/// terrain state match [`SharedCovering`], avoiding duplicate quadtree walks in Cleanup.
pub(crate) fn matching_tight_tiles<'a>(
    world: &'a World,
    style: &Style,
    view_state: &ViewState,
    level: ZoomLevel,
) -> Option<&'a [WorldTileCoords]> {
    let eye = world.resources.get::<EyeInFrame>().copied();
    if let Some(eye) = eye.filter(|e| e.index > 0) {
        return world
            .resources
            .get::<SharedCovering>()
            .filter(|shared| shared.frame == Some(eye.frame))
            .map(|shared| {
                shared
                    .tiles
                    .as_ref()
                    .map_or(&[][..], |(tiles, _)| tiles.as_slice())
            });
    }
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let view_proj_bits = frame_view_proj_bits(world, style, view_state);
    let style_rev = style.revision();
    let terrain_key = terrain_index_key(world);
    world
        .resources
        .get::<SharedCovering>()
        .filter(|shared| {
            shared.frame == eye.map(|e| e.frame)
                && shared.host_frame == host_frame
                && shared.style_rev == style_rev
                && shared.level == level
                && shared.view_proj_bits == view_proj_bits
                && shared.terrain_key == terrain_key
        })
        .map(|shared| {
            shared
                .tiles
                .as_ref()
                .map_or(&[][..], |(tiles, _)| tiles.as_slice())
        })
}

pub(crate) fn terrain_index_key(world: &World) -> (u64, u64, usize) {
    world
        .resources
        .get::<crate::terrain::coverage::TerrainCoverageIndex>()
        .map_or((0, 0, 0), |idx| {
            (
                idx.min_elevation().to_bits(),
                idx.max_elevation().to_bits(),
                idx.dem_bounds_len(),
            )
        })
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
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let multi_active = world
        .resources
        .get::<MultiViewExtractCovering>()
        .is_some_and(|m| m.active);
    let view_proj_bits = frame_view_proj_bits(world, style, view_state);
    let style_rev = style.revision();
    let terrain_key = terrain_index_key(world);
    if !multi_active && eye.is_none_or(|e| e.index == 0) {
        if let Some(shared) = world.resources.get::<SharedCovering>().filter(|shared| {
            shared.frame == eye.map(|e| e.frame)
                && shared.host_frame == host_frame
                && shared.style_rev == style_rev
                && shared.level == level
                && shared.view_proj_bits == view_proj_bits
                && shared.terrain_key == terrain_key
        }) {
            return Ok((shared.region(), shared.raster.clone()));
        }
    }
    let cached_tight_region = if !multi_active && eye.is_none_or(|eye| eye.index == 0) {
        world
            .resources
            .get::<FrameLooseCovering>()
            .filter(|cache| cache.host_frame == host_frame)
            .and_then(|cache| cache.view_tiles.as_ref())
            .filter(|(_, padding, cached_level)| {
                *padding == ViewStatePadding::Tight && *cached_level == level
            })
            .map(|(tiles, _, _)| {
                tiles
                    .as_ref()
                    .map(|tiles| ViewRegion::from_tiles(tiles.to_vec(), level, tiles.len().max(1)))
            })
    } else {
        None
    };
    let region = match cached_tight_region {
        Some(cached) => cached,
        None => {
            view_region_for_projection(style, view_state, world, level, ViewStatePadding::Tight)?
        }
    };
    let cached_raster = if !multi_active && eye.is_none_or(|eye| eye.index == 0) {
        world
            .resources
            .get::<FrameLooseCovering>()
            .filter(|cache| cache.host_frame == host_frame)
            .and_then(|cache| cache.raster_regions.as_ref())
            .filter(|(_, padding, cached_terrain)| {
                *padding == ViewStatePadding::Tight && *cached_terrain == terrain_key
            })
            .map(|(regions, _, _)| regions.clone())
    } else {
        None
    };
    let raster = match cached_raster {
        Some(cached) => cached,
        None => raster_source_regions(style, view_state, world, ViewStatePadding::Tight)?,
    };
    if eye.is_none_or(|eye| eye.index == 0) {
        world.resources.insert(SharedCovering {
            frame: eye.map(|eye| eye.frame),
            host_frame,
            style_rev,
            level,
            view_proj_bits,
            terrain_key,
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

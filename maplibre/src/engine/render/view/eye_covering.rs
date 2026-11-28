//! One tile covering for both eyes of a frame.
//!
//! Tile selection and content ingestion happen once per stereo frame. Each eye then draws
//! that selection with its own projection, so a tile cannot change level or content between
//! the left and right images.

use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel},
    io::tile_sources::RasterSourceId,
    render::{
        projection::{view_region_for_projection, ProjectionStateError},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
};
mod lod_snapshot;
mod raster;
pub(crate) use lod_snapshot::snapshot_lod_history;
pub use raster::raster_source_regions;

pub use crate::render::frame_signals::EyeInFrame;
pub(crate) use crate::render::projection::FrameLodHistory;

/// Populates `out` with the wrapped covering tiles used for the globe background, reusing `out`'s
/// existing heap allocation.
pub(crate) fn fill_globe_tiles(world: &World, out: &mut Vec<WorldTileCoords>) {
    out.clear();
    if let Some(drawn) = drawn_tiles(world) {
        out.extend(
            drawn
                .iter()
                .copied()
                .filter(|coords| coords.wrapped().is_some()),
        );
    }
    if out.is_empty() {
        out.push(WorldTileCoords::default());
    }
}

/// The tiles the globe's background is drawn on: the frame's covering, so that the ground
/// near an eye standing on the globe follows the sphere as closely as the tiles drawn there;
/// the world tile while no covering is selected.
pub fn globe_tiles(world: &World) -> Vec<WorldTileCoords> {
    let mut tiles = Vec::new();
    fill_globe_tiles(world, &mut tiles);
    tiles
}

/// Identifier of a registered viewport slot in a multi-view frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewSlotId(u32);

impl ViewSlotId {
    /// Raw zero-based slot index.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl From<u32> for ViewSlotId {
    fn from(raw: u32) -> Self {
        Self(raw)
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
    pub(crate) tight_view_tiles: Option<CachedViewTiles>,
    pub(crate) raster_regions: Option<(RasterCoverings, ViewStatePadding, (u64, u64, usize))>,
}

impl FrameLooseCovering {
    pub(crate) fn for_host_frame(host_frame: u64) -> Self {
        Self {
            host_frame,
            view_tiles: None,
            tight_view_tiles: None,
            raster_regions: None,
        }
    }
}

/// Total number of eye passes executing in the current [`XrFrame`](crate::render::xr::XrFrame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EyePassCount(pub usize);

/// Tiles per raster source, as [`raster_source_regions`] selects them.
pub type RasterCoverings = Vec<(RasterSourceId, Vec<WorldTileCoords>)>;

/// The drawn tiles a frame selected: the first eye's, which later eyes of the frame reuse, or
/// the map camera's own.
#[derive(Clone, Debug)]
pub struct SharedCovering {
    /// The stereo frame whose eyes share the selection; none for a map camera's frame.
    frame: Option<u64>,
    view_slot: Option<ViewSlotId>,
    host_frame: u64,
    style_rev: u64,
    level: ZoomLevel,
    view_proj_bits: [[u64; 4]; 4],
    terrain_key: (u64, u64, usize),
    pub(super) tiles: Option<(Arc<[WorldTileCoords]>, ZoomLevel)>,
    pub(super) raster: RasterCoverings,
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
        self.tiles.as_ref().map(|(tiles, _)| &**tiles)
    }
}

impl AsRef<[(RasterSourceId, Vec<WorldTileCoords>)]> for SharedCovering {
    /// The tiles each raster source selected for the frame.
    fn as_ref(&self) -> &[(RasterSourceId, Vec<WorldTileCoords>)] {
        &self.raster
    }
}

/// The tiles the frame's covering selected, each once, or `None` before a covering is selected
/// or when it selected no region.
pub(crate) fn drawn_tiles(world: &World) -> Option<&[WorldTileCoords]> {
    world.resources.get::<SharedCovering>()?.tiles()
}

pub(crate) fn frame_view_proj_bits(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> [[u64; 4]; 4] {
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
    let query = CoveringQuery::from_frame(style, view_state, world, level);
    matching_shared_covering(world, &query)
        .map(|shared| shared.tiles.as_ref().map_or(&[][..], |(tiles, _)| &**tiles))
}

pub(crate) fn terrain_index_key(world: &World) -> (u64, u64, usize) {
    world
        .resources
        .get::<crate::render::projection::TerrainElevationBridge>()
        .and_then(|bridge| (bridge.get_index)(world))
        .map_or((0, 0, 0), |idx| idx.index_key())
}

type CachedTightRegion = (Option<Arc<[WorldTileCoords]>>, Option<ViewRegion>);

fn tight_cached_region(
    world: &World,
    host_frame: u64,
    level: ZoomLevel,
    can_use_cache: bool,
) -> Option<CachedTightRegion> {
    if !can_use_cache {
        return None;
    }
    let cache = world
        .resources
        .get::<FrameLooseCovering>()
        .filter(|cache| cache.host_frame == host_frame)?;
    cache
        .tight_view_tiles
        .as_ref()
        .or(cache.view_tiles.as_ref())
        .filter(|(_, padding, cached_level)| {
            *padding == ViewStatePadding::Tight && *cached_level == level
        })
        .map(|(tiles, _, _)| match tiles.as_ref() {
            Some(arc) => (
                Some(Arc::clone(arc)),
                Some(ViewRegion::from_tiles(
                    arc.to_vec(),
                    level,
                    arc.len().max(1),
                )),
            ),
            None => (None, None),
        })
}

fn tight_cached_raster(
    world: &World,
    host_frame: u64,
    terrain_key: (u64, u64, usize),
    can_use_cache: bool,
) -> Option<RasterCoverings> {
    if !can_use_cache {
        return None;
    }
    world
        .resources
        .get::<FrameLooseCovering>()
        .filter(|cache| cache.host_frame == host_frame)
        .and_then(|cache| cache.raster_regions.as_ref())
        .filter(|(_, padding, cached_terrain)| {
            *padding == ViewStatePadding::Tight && *cached_terrain == terrain_key
        })
        .map(|(regions, _, _)| regions.clone())
}

struct CoveringQuery {
    eye: Option<EyeInFrame>,
    view_slot: Option<ViewSlotId>,
    host_frame: u64,
    style_rev: u64,
    level: ZoomLevel,
    view_proj_bits: [[u64; 4]; 4],
    terrain_key: (u64, u64, usize),
    multi_active: bool,
}

impl CoveringQuery {
    fn from_frame(style: &Style, view_state: &ViewState, world: &World, level: ZoomLevel) -> Self {
        let eye = world.resources.get::<EyeInFrame>().copied();
        let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
        let view_slot = world.resources.get::<CurrentViewSlot>().and_then(|s| s.0);
        let multi_active = world
            .resources
            .get::<MultiViewExtractCovering>()
            .is_some_and(|m| m.active);
        Self {
            eye,
            view_slot,
            host_frame,
            style_rev: style.revision(),
            level,
            view_proj_bits: frame_view_proj_bits(world, style, view_state),
            terrain_key: terrain_index_key(world),
            multi_active,
        }
    }

    fn is_primary_eye(&self) -> bool {
        self.eye.is_none_or(|e| e.index == 0)
    }

    fn can_use_loose_cache(&self) -> bool {
        !self.multi_active && self.is_primary_eye()
    }

    fn matches(&self, shared: &SharedCovering) -> bool {
        if let Some(eye) = self.eye.filter(|eye| eye.index > 0) {
            return shared.frame == Some(eye.frame);
        }
        shared.frame == self.eye.map(|e| e.frame)
            && shared.view_slot == self.view_slot
            && shared.host_frame == self.host_frame
            && shared.style_rev == self.style_rev
            && shared.level == self.level
            && shared.view_proj_bits == self.view_proj_bits
            && shared.terrain_key == self.terrain_key
    }
}

fn matching_shared_covering<'w>(
    world: &'w World,
    query: &CoveringQuery,
) -> Option<&'w SharedCovering> {
    world
        .resources
        .get::<SharedCovering>()
        .filter(|shared| query.matches(shared))
}

type CoveringParts = (
    Option<Arc<[WorldTileCoords]>>,
    Option<ViewRegion>,
    RasterCoverings,
);

fn compute_covering_parts(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    query: &CoveringQuery,
) -> Result<CoveringParts, ProjectionStateError> {
    let can_use_loose_cache = query.can_use_loose_cache();
    let (cached_arc, region) =
        match tight_cached_region(world, query.host_frame, query.level, can_use_loose_cache) {
            Some((arc, region)) => (arc, region),
            None => (
                None,
                view_region_for_projection(
                    style,
                    view_state,
                    world,
                    query.level,
                    ViewStatePadding::Tight,
                )?,
            ),
        };
    let raster = match tight_cached_raster(
        world,
        query.host_frame,
        query.terrain_key,
        can_use_loose_cache,
    ) {
        Some(cached) => cached,
        None => raster_source_regions(style, view_state, world, ViewStatePadding::Tight)?,
    };
    Ok((cached_arc, region, raster))
}

fn region_tiles(
    cached_arc: Option<Arc<[WorldTileCoords]>>,
    region: Option<&ViewRegion>,
) -> Option<(Arc<[WorldTileCoords]>, ZoomLevel)> {
    region.map(|region| {
        let arc = cached_arc.unwrap_or_else(|| Arc::from(region.copies().collect::<Vec<_>>()));
        (arc, region.zoom_level())
    })
}

/// Fast covering result that keeps shared covering `Arc<[WorldTileCoords]>` lazy so fast-key
/// hits skip building `ViewRegion`.
pub(crate) struct FastDrawnCovering {
    tiles: Option<(Arc<[WorldTileCoords]>, ZoomLevel)>,
    cached_region: Option<ViewRegion>,
    pub(crate) raster: RasterCoverings,
}

impl FastDrawnCovering {
    pub(crate) fn has_tiles(&self) -> bool {
        self.tiles.is_some()
    }

    pub(crate) fn tiles_arc(&self) -> Option<&Arc<[WorldTileCoords]>> {
        self.tiles.as_ref().map(|(tiles, _)| tiles)
    }

    pub(crate) fn into_region(self) -> Option<ViewRegion> {
        self.cached_region.or_else(|| {
            self.tiles
                .map(|(tiles, level)| ViewRegion::from_tiles(tiles.to_vec(), level, tiles.len()))
        })
    }
}

impl CoveringQuery {
    fn store_shared_tiles(
        &self,
        world: &mut World,
        tiles: Option<(Arc<[WorldTileCoords]>, ZoomLevel)>,
        raster: RasterCoverings,
    ) {
        let next = SharedCovering {
            frame: self.eye.map(|eye| eye.frame),
            view_slot: self.view_slot,
            host_frame: self.host_frame,
            style_rev: self.style_rev,
            level: self.level,
            view_proj_bits: self.view_proj_bits,
            terrain_key: self.terrain_key,
            tiles,
            raster,
        };
        if let Some(slot) = world.resources.get_mut::<SharedCovering>() {
            *slot = next;
        } else {
            world.resources.insert(next);
        }
    }
}

/// Ensures [`SharedCovering`] holds the current frame's covering when it tracks the primary
/// eye, without cloning the covering back out when `SharedCovering` is current. Returns `Some`
/// only for a secondary eye whose covering was not already cached on [`SharedCovering`].
pub(crate) fn ensure_drawn_covering_for_raster(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    level: ZoomLevel,
) -> Result<Option<RasterCoverings>, ProjectionStateError> {
    let query = CoveringQuery::from_frame(style, view_state, world, level);
    if matching_shared_covering(world, &query).is_some() {
        return Ok(None);
    }
    let (cached_arc, region, raster) = compute_covering_parts(style, view_state, world, &query)?;
    if query.is_primary_eye() {
        let tiles = region_tiles(cached_arc, region.as_ref());
        query.store_shared_tiles(world, tiles, raster);
        return Ok(None);
    }
    Ok(Some(raster))
}

/// Returns [`FastDrawnCovering`] without building a `ViewRegion` when [`SharedCovering`] hits.
pub(crate) fn drawn_covering_fast(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    level: ZoomLevel,
) -> Result<FastDrawnCovering, ProjectionStateError> {
    let query = CoveringQuery::from_frame(style, view_state, world, level);
    if let Some(shared) = matching_shared_covering(world, &query) {
        return Ok(FastDrawnCovering {
            tiles: shared.tiles.clone(),
            cached_region: None,
            raster: shared.raster.clone(),
        });
    }
    let (cached_arc, region, raster) = compute_covering_parts(style, view_state, world, &query)?;
    let tiles = region_tiles(cached_arc, region.as_ref());
    if query.is_primary_eye() {
        query.store_shared_tiles(world, tiles.clone(), raster.clone());
    }
    Ok(FastDrawnCovering {
        tiles,
        cached_region: region,
        raster,
    })
}

#[cfg(test)]
mod tests;

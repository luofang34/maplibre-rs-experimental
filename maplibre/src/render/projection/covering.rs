//! Visible requests and speculative coverage under the active projection.
use super::{globe_camera_for_view, ProjectionStateError};
use crate::{
    coords::{ViewRegion, WorldTileCoords, ZoomLevel, TILE_SIZE},
    io::tile_sources::{covering_zoom, TileKind},
    projection::{
        globe::{
            covering::{TileElevationProvider, TileElevationRange},
            covering_tiles::{
                covering_tiles_with_history, elevation_for_tile_culling, GlobeCoveringOptions,
                SourceZoomRange, ZoomRounding,
            },
        },
        lod_history::LodHistory,
        mercator::{
            covering_tiles::covering_tiles_with_history as mercator_covering_tiles,
            MercatorCoveringOptions,
        },
        ProjectionType,
    },
    render::{
        eye_covering::FrameLodHistory,
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        xr::PrefetchView,
    },
    style::{source::Source, Style},
    tcs::world::World,
    terrain::coverage::{IndexedTileElevation, TerrainCoverageIndex},
};
use std::collections::HashSet;
const ASSUMED_MAX_FEATURE_HEIGHT_METERS: f64 = 500.0;
const MAX_MERCATOR_HORIZON_DEGREES: f64 = 89.25;
const TILE_CULLING_HORIZON_ONSET_DEGREES: f64 = 15.0;
/// Which tiles a covering asks for: the nominal level, the fractional zoom the distance rule
/// starts from, and the rounding and zoom range of the source the tiles come from. The view
/// and vector sources use the map zoom floored; a raster source adjusts the zoom for its tile
/// size and rounds, as GL JS gives every source its own tile manager.
#[derive(Clone, Copy, Debug)]
pub struct CoveringRequest {
    /// Level selected where the zoom does not vary per tile.
    pub level: ZoomLevel,
    /// Fractional zoom at the view center, already adjusted for the source tile size.
    pub requested_zoom: f64,
    /// How a per-tile zoom becomes a level.
    pub rounding: ZoomRounding,
    /// Levels the source serves.
    pub zoom_range: SourceZoomRange,
}

impl CoveringRequest {
    /// The request for the view tiles at `level`.
    pub fn view(level: ZoomLevel, zoom: f64) -> Self {
        Self {
            level,
            requested_zoom: zoom,
            rounding: ZoomRounding::Floor,
            zoom_range: SourceZoomRange::default(),
        }
    }

    /// The request for a raster source of `tile_size` pixels: 256-pixel tiles sit one level
    /// below the 512-pixel view tiles, and the level is rounded rather than floored.
    pub fn raster_source(
        zoom: f64,
        tile_size: f64,
        minzoom: Option<u8>,
        maxzoom: Option<u8>,
    ) -> Self {
        let zoom_range = SourceZoomRange::from_style(minzoom, maxzoom);
        let level = ZoomLevel::new(covering_zoom(zoom, TileKind::Raster, tile_size));
        Self {
            level: zoom_range.cap(level),
            requested_zoom: zoom + (TILE_SIZE / tile_size).log2(),
            rounding: ZoomRounding::Round,
            zoom_range,
        }
    }
}

/// Selects the visible region using the projection declared by the current style.
pub fn view_region_for_projection(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    visible_level: ZoomLevel,
    padding: ViewStatePadding,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let mut region = covering_region(
        style,
        view_state,
        world,
        CoveringRequest::view(visible_level, view_state.zoom().value()),
        padding,
    )?;
    if padding != ViewStatePadding::Loose {
        return Ok(region);
    }
    if view_state.has_external_view() && region.is_some() {
        let visible = covering_region(
            style,
            view_state,
            world,
            CoveringRequest::view(visible_level, view_state.zoom().value()),
            ViewStatePadding::Tight,
        )?;
        region = union_regions(visible, region, visible_level, SURROUND_MAX_TILES);
    }
    // A flight in progress requests the frame it is heading for, at that frame's own level.
    let Some(ahead) = world
        .resources
        .get::<PrefetchView>()
        .and_then(|prefetch| prefetch.view_state.as_ref())
    else {
        return Ok(region);
    };
    let level = ahead.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let destination = covering_region(
        style,
        ahead,
        world,
        CoveringRequest::view(level, ahead.zoom().value()),
        ViewStatePadding::Tight,
    )?;
    Ok(union_regions(
        region,
        destination,
        visible_level,
        PREFETCH_MAX_TILES,
    ))
}

/// The tiles each raster source used by a visible layer covers, following that source's tile
/// size, rounding and zoom range through the same covering as the view.
pub fn raster_source_regions(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    padding: ViewStatePadding,
) -> Result<Vec<(String, Vec<WorldTileCoords>)>, ProjectionStateError> {
    let zoom = view_state.zoom().value();
    let mut regions = Vec::new();
    for (name, source) in &style.sources {
        // Imagery is read by raster layers, elevation tiles by the DEM-shaded layers.
        let (tile_size, minzoom, maxzoom, layer_types): (f64, _, _, &[&str]) = match source {
            Source::Raster(raster) => (
                raster.tile_size.map_or(TILE_SIZE, f64::from),
                raster.minzoom,
                raster.maxzoom,
                &["raster"],
            ),
            Source::RasterDem(dem) => (
                f64::from(dem.tile_size),
                dem.minzoom,
                dem.maxzoom,
                &["hillshade", "color-relief"],
            ),
            _ => continue,
        };
        let used = style.layers.iter().any(|layer| {
            layer_types.contains(&layer.type_.as_str())
                && layer.source.as_deref() == Some(name)
                && layer.is_visible_at(zoom)
        });
        if !used {
            continue;
        }
        let request = CoveringRequest::raster_source(zoom, tile_size, minzoom, maxzoom);
        let history = world
            .resources
            .get::<FrameLodHistory>()
            .and_then(|h| h.raster.get(name));
        let tiles =
            covering_region_with_history(style, view_state, world, request, padding, history)?
                .map_or_else(Vec::new, |region| {
                    region
                        .iter()
                        .filter(|coords| coords.build_quad_key().is_some())
                        .collect()
                });
        regions.push((name.clone(), tiles));
    }
    Ok(regions)
}

const MERCATOR: ProjectionType = ProjectionType::Mercator;
/// How far an eye's surround covering reaches to every side, as a multiple of its height:
/// the ground a head turn reveals nearby. Farther ground comes as coarse tiles the frame
/// shares between directions.
const SURROUND_REACH: f64 = 2.0;
/// Tiles a request may hold once the surround is added to what the eye sees; a flight
/// through several zoom levels lands every level's request, so each stays small.
const SURROUND_MAX_TILES: usize = 128;

/// Selects the tiles of a request under the projection declared by the current style.
pub fn covering_region(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    request: CoveringRequest,
    padding: ViewStatePadding,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let history = world.resources.get::<FrameLodHistory>().map(|h| &h.view);
    covering_region_with_history(style, view_state, world, request, padding, history)
}

fn covering_region_with_history(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    request: CoveringRequest,
    padding: ViewStatePadding,
    history: Option<&LodHistory>,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let history = history.filter(|_| view_state.has_external_view());
    if !request.zoom_range.serves(request.level) {
        return Ok(None);
    }
    // A loose covering is a request for tiles; an external eye may ask for it to reach beyond
    // the frame so tiles are ready where the head turns next, once the eye has settled.
    let widened = match padding {
        ViewStatePadding::Loose if view_state.eye_settled() => {
            view_state.overscanned(view_state.request_overscan())
        }
        ViewStatePadding::Loose | ViewStatePadding::Tight => None,
    };
    let view_state = widened.as_ref().unwrap_or(view_state);
    let projection_type = style
        .projection
        .as_ref()
        .map_or(&MERCATOR, |specification| &specification.projection_type);
    let uses_globe = projection_type.uses_globe_rendering(view_state.zoom().value());
    if !uses_globe {
        let region = mercator_view_region(style, view_state, world, request, padding, history)?;
        // A head turns faster than tiles arrive, so an eye's requests also cover what
        // surrounds it on the ground.
        let surround = match padding {
            ViewStatePadding::Loose => view_state.surround(SURROUND_REACH, projection_type),
            ViewStatePadding::Tight => None,
        };
        let Some(surround) = surround else {
            return Ok(region);
        };
        let around = mercator_view_region(style, &surround, world, request, padding, history)?;
        return Ok(union_regions(
            region,
            around,
            request.level,
            SURROUND_MAX_TILES,
        ));
    }
    globe_view_region(style, view_state, world, request, padding, history)
}

fn globe_view_region(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    request: CoveringRequest,
    padding: ViewStatePadding,
    history: Option<&LodHistory>,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let visible_level = request.level;
    let camera = globe_camera_for_view(view_state)?;
    let options = GlobeCoveringOptions {
        zoom: visible_level,
        requested_zoom: request.requested_zoom,
        variable_zoom: u8::from(visible_level) > 4,
        rounding: request.rounding,
        zoom_range: request.zoom_range,
        padding: match padding {
            ViewStatePadding::Loose => 1,
            ViewStatePadding::Tight => 0,
        },
        max_tiles: 512,
    };
    let fallback = TileElevationRange {
        min_meters: 0.0,
        max_meters: elevation_for_tile_culling(&camera, view_state.center_elevation()),
    };
    let elevation = tile_elevation(style, world, fallback);
    let tiles = covering_tiles_with_history(&camera, options, elevation.as_ref(), history)
        .map_err(|source| ProjectionStateError::GlobeCovering { source })?;
    Ok(Some(ViewRegion::from_tiles(tiles, visible_level, 512)))
}

/// Tiles a request may hold once a flight's destination is added to what the eye sees. The
/// destination's covering comes nearest first, so this many tiles are the ground around
/// the arrival gaze; the rest load after landing, when they can be drawn.
const PREFETCH_MAX_TILES: usize = 96;

/// The tiles of both regions, those of `primary` first, as one region at `level`, at most
/// `max_tiles` of them, or all primary tiles when that view alone exceeds the limit.
fn union_regions(
    primary: Option<ViewRegion>,
    secondary: Option<ViewRegion>,
    level: ZoomLevel,
    max_tiles: usize,
) -> Option<ViewRegion> {
    match (primary, secondary) {
        (None, other) | (other, None) => other,
        (Some(primary), Some(secondary)) => {
            // Speculative padding must never evict tiles the primary view actually draws.
            let max_tiles = max_tiles.max(primary.iter().count());
            let mut seen = HashSet::new();
            let tiles: Vec<WorldTileCoords> = primary
                .iter()
                .chain(secondary.iter())
                .filter(|coords| seen.insert(*coords))
                .collect();
            Some(ViewRegion::from_tiles(tiles, level, max_tiles))
        }
    }
}

/// Culling bounds per tile: the loaded DEM's range where terrain has one, else `fallback`,
/// the range GL JS assumes for tiles without elevation data.
fn tile_elevation<'a>(
    style: &Style,
    world: &'a World,
    fallback: TileElevationRange,
) -> Box<dyn TileElevationProvider + 'a> {
    match world.resources.get::<TerrainCoverageIndex>() {
        Some(index) if style.terrain.is_some() => {
            Box::new(IndexedTileElevation { index, fallback })
        }
        _ => Box::new(fallback),
    }
}

/// Selects Mercator tiles: the ground-plane bounding box for flat views, or frustum culling
/// with distance-based zoom once terrain is on or the pitch exceeds GL JS's constant-zoom limit.
fn mercator_view_region(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    request: CoveringRequest,
    padding: ViewStatePadding,
    history: Option<&LodHistory>,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let visible_level = request.level;
    let pitch_degrees = view_state.camera().get_pitch().0.to_degrees().abs();
    let fov_degrees = view_state.field_of_view().0.to_degrees();
    let needs_frustum = view_state.has_external_view()
        || style.terrain.is_some()
        || pitch_degrees > max_constant_zoom_pitch(fov_degrees);
    if !needs_frustum {
        return Ok(view_state.create_view_region(visible_level, padding));
    }
    let elevation = tile_elevation(
        style,
        world,
        mercator_elevation_range(view_state, pitch_degrees, fov_degrees),
    );
    let tiles = mercator_covering_tiles(
        view_state,
        MercatorCoveringOptions {
            zoom: visible_level,
            requested_zoom: request.requested_zoom,
            variable_zoom: true,
            rounding: request.rounding,
            zoom_range: request.zoom_range,
            padding: match padding {
                ViewStatePadding::Loose => 1,
                ViewStatePadding::Tight => 0,
            },
            max_tiles: 512,
        },
        elevation.as_ref(),
        history,
    )
    .map_err(|source| ProjectionStateError::MercatorCovering { source })?;
    Ok(Some(ViewRegion::from_tiles(tiles, visible_level, 512)))
}

/// Pitch above which GL JS varies zoom per tile: 78.5 degrees minus half the field of view,
/// clamped to 60 degrees.
pub(super) fn max_constant_zoom_pitch(fov_degrees: f64) -> f64 {
    (78.5 - fov_degrees / 2.0).clamp(0.0, 60.0)
}

/// Elevation range of the tile boxes without terrain: the center elevation plus GL JS's
/// feature-height allowance near the horizon.
pub(super) fn mercator_elevation_range(
    view_state: &ViewState,
    pitch_degrees: f64,
    fov_degrees: f64,
) -> TileElevationRange {
    let bottom_edge_above_horizontal =
        MAX_MERCATOR_HORIZON_DEGREES - pitch_degrees - fov_degrees * 0.5;
    let proximity = ((TILE_CULLING_HORIZON_ONSET_DEGREES - bottom_edge_above_horizontal)
        / TILE_CULLING_HORIZON_ONSET_DEGREES)
        .clamp(0.0, 1.0);
    let elevation = view_state.center_elevation() + proximity * ASSUMED_MAX_FEATURE_HEIGHT_METERS;
    TileElevationRange {
        min_meters: elevation.min(0.0),
        max_meters: elevation.max(0.0),
    }
}

#[cfg(test)]
mod tests;

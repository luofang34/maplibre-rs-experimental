use crate::{
    coords::{TileCoords, WorldTileCoords},
    projection::globe::{
        camera::GlobeCameraState,
        covering::{TileElevationProvider, TileElevationRange},
    },
    style::{layer::LayerPaint, Style},
    tcs::{pyramid::TilePyramid, tiles::Tiles, world::World},
};

#[inline]
fn dem_tile_coords_for(
    coords: WorldTileCoords,
    minzoom: u8,
    maxzoom: u8,
) -> Option<WorldTileCoords> {
    let zoom = u8::from(coords.z).saturating_sub(1).min(maxzoom);
    if zoom < minzoom {
        return None;
    }
    let mut current = coords;
    while u8::from(current.z) > zoom {
        current = current.get_parent()?;
    }
    Some(current)
}

/// Borrowed terrain DEM elevation bounds enabling direct static-dispatch lookups during covering.
#[derive(Clone, Copy)]
pub struct ElevationBoundsView<'a> {
    /// Interior elevation bounds of every loaded DEM tile, without exaggeration.
    pub dem_bounds: &'a TilePyramid<(f64, f64)>,
    /// Vertical terrain exaggeration multiplier.
    pub exaggeration: f64,
    /// Minimum zoom of the active DEM source.
    pub minzoom: u8,
    /// Maximum zoom of the active DEM source.
    pub maxzoom: u8,
    /// North polar cap datum elevation in metres.
    pub north_pole_meters: f32,
    /// South polar cap datum elevation in metres.
    pub south_pole_meters: f32,
}

impl ElevationBoundsView<'_> {
    #[inline]
    fn tile_elevation_range(&self, coords: WorldTileCoords) -> Option<TileElevationRange> {
        let current = dem_tile_coords_for(coords, self.minzoom, self.maxzoom)?;
        let (_, &(min, max)) = self.dem_bounds.nearest_ancestor_or_self(current)?;
        let mut min_meters = min * self.exaggeration;
        let mut max_meters = max * self.exaggeration;
        let max_row = 1_i32
            .checked_shl(u32::from(u8::from(coords.z)))
            .unwrap_or(1)
            - 1;
        if coords.y == 0 && self.north_pole_meters != 0.0 {
            let pole = f64::from(self.north_pole_meters) * self.exaggeration;
            min_meters = min_meters.min(pole);
            max_meters = max_meters.max(pole);
        }
        if coords.y == max_row && self.south_pole_meters != 0.0 {
            let pole = f64::from(self.south_pole_meters) * self.exaggeration;
            min_meters = min_meters.min(pole);
            max_meters = max_meters.max(pole);
        }
        Some(TileElevationRange {
            min_meters,
            max_meters,
        })
    }
}

/// Elevation lookup interface implemented by terrain's coverage index so projection covering
/// and culling stay decoupled from `crate::terrain`.
pub trait TerrainElevationSource {
    /// Borrowed DEM bounds view for static-dispatch per-tile elevation queries.
    fn elevation_bounds_view(&self) -> ElevationBoundsView<'_>;
    /// Exaggerated ground elevation at the camera nadir using the finest loaded DEM.
    fn nadir_ground_elevation(&self, tiles: &Tiles, camera: &GlobeCameraState) -> f64;
    /// Compact cache key `(min_elevation_bits, max_elevation_bits, dem_bounds_len)`.
    fn index_key(&self) -> (u64, u64, usize);
}

/// Callback bridge registered by terrain initialization into `world.resources`.
#[derive(Clone, Copy)]
pub struct TerrainElevationBridge {
    /// Looks up the active [`TerrainElevationSource`] from `world`, if initialized.
    pub get_index: for<'a> fn(&'a World) -> Option<&'a dyn TerrainElevationSource>,
    /// Returns the drawn terrain surface tiles recorded for the current frame.
    pub drawn_surface_tiles: for<'a> fn(&'a World) -> &'a [WorldTileCoords],
}

/// Conservative upper ceiling (`20,000 m`, above `FL600`) added to view-tile culling boxes when a
/// style contains 3D translucent extrusions or elevated 3D lines (`line-elevation`).
const ELEVATED_3D_HEADROOM_METERS: f64 = 20_000.0;

/// Upper ceiling (`1,000 m`) added to view-tile culling boxes for standard opaque `fill-extrusion`
/// layers so tall buildings near the camera or horizon at steep pitch are not culled.
const OPAQUE_EXTRUSION_HEADROOM_METERS: f64 = 1_000.0;

pub(crate) fn style_elevated_headroom_meters(style: &Style) -> f64 {
    let mut headroom: f64 = 0.0;
    for layer in &style.layers {
        match &layer.paint {
            Some(LayerPaint::FillExtrusion(paint))
                if paint.fill_extrusion_translucent == Some(true) =>
            {
                return ELEVATED_3D_HEADROOM_METERS;
            }
            Some(LayerPaint::FillExtrusion(_)) if !layer.is_hidden() => {
                headroom = headroom.max(OPAQUE_EXTRUSION_HEADROOM_METERS);
            }
            Some(LayerPaint::Line(paint)) if paint.line_elevation.is_some() => {
                return ELEVATED_3D_HEADROOM_METERS;
            }
            _ => {}
        }
    }
    headroom
}

pub(super) struct BorrowedTileElevation<'a> {
    view: ElevationBoundsView<'a>,
    fallback: TileElevationRange,
}

impl TileElevationProvider for BorrowedTileElevation<'_> {
    #[inline]
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange {
        let coords = WorldTileCoords {
            x: tile.x as i32,
            y: tile.y as i32,
            z: tile.z,
        };
        self.view
            .tile_elevation_range(coords)
            .unwrap_or(self.fallback)
    }
}

pub(super) struct HeadroomTileElevation<P> {
    inner: P,
    headroom_meters: f64,
}

impl<P: TileElevationProvider> TileElevationProvider for HeadroomTileElevation<P> {
    #[inline]
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange {
        let mut range = self.inner.elevation_range(tile);
        range.max_meters += self.headroom_meters;
        range
    }
}

/// Stack-allocated tile elevation provider avoiding a heap `Box` on every covering walk.
pub(super) enum TileElevation<'a> {
    Indexed(HeadroomTileElevation<BorrowedTileElevation<'a>>),
    Constant(TileElevationRange),
}

impl TileElevationProvider for TileElevation<'_> {
    #[inline]
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange {
        match self {
            Self::Indexed(indexed) => indexed.elevation_range(tile),
            Self::Constant(range) => *range,
        }
    }
}

/// Culling bounds per tile: the loaded DEM's range where terrain has one, else `fallback`,
/// widened upward by `headroom_meters` when the style renders elevated 3D volumes or lines.
pub(super) fn tile_elevation<'a>(
    style: &Style,
    world: &'a World,
    fallback: TileElevationRange,
    headroom_meters: f64,
) -> TileElevation<'a> {
    if style.terrain.is_none() {
        return TileElevation::Constant(TileElevationRange {
            min_meters: fallback.min_meters,
            max_meters: fallback.max_meters + headroom_meters,
        });
    }
    match world
        .resources
        .get::<TerrainElevationBridge>()
        .and_then(|bridge| (bridge.get_index)(world))
    {
        Some(index) => TileElevation::Indexed(HeadroomTileElevation {
            inner: BorrowedTileElevation {
                view: index.elevation_bounds_view(),
                fallback,
            },
            headroom_meters,
        }),
        None => TileElevation::Constant(TileElevationRange {
            min_meters: fallback.min_meters,
            max_meters: fallback.max_meters + headroom_meters,
        }),
    }
}

pub(super) fn nadir_ground_elevation(
    style: &Style,
    world: &World,
    camera: &GlobeCameraState,
) -> f64 {
    if style.terrain.is_none() {
        return 0.0;
    }
    world
        .resources
        .get::<TerrainElevationBridge>()
        .and_then(|bridge| (bridge.get_index)(world))
        .map_or(0.0, |index| {
            index.nadir_ground_elevation(&world.tiles, camera)
        })
}

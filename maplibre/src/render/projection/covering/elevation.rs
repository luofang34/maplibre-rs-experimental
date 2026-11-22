use cgmath::InnerSpace;

use crate::{
    coords::TileCoords,
    projection::globe::{
        camera::GlobeCameraState,
        covering::{TileElevationProvider, TileElevationRange},
    },
    style::{layer::LayerPaint, Style},
    tcs::world::World,
    terrain::coverage::{DemDetail, IndexedTileElevation, TerrainCoverageIndex},
};

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

pub(super) struct HeadroomTileElevation<P> {
    inner: P,
    headroom_meters: f64,
}

impl<P: TileElevationProvider> TileElevationProvider for HeadroomTileElevation<P> {
    fn elevation_range(&self, tile: TileCoords) -> TileElevationRange {
        let mut range = self.inner.elevation_range(tile);
        range.max_meters += self.headroom_meters;
        range
    }
}

/// Stack-allocated tile elevation provider avoiding a heap `Box` on every covering walk.
pub(super) enum TileElevation<'a> {
    Indexed(HeadroomTileElevation<IndexedTileElevation<'a>>),
    Constant(TileElevationRange),
}

impl TileElevationProvider for TileElevation<'_> {
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
    match world.resources.get::<TerrainCoverageIndex>() {
        Some(index) if style.terrain.is_some() => TileElevation::Indexed(HeadroomTileElevation {
            inner: IndexedTileElevation { index, fallback },
            headroom_meters,
        }),
        _ => TileElevation::Constant(TileElevationRange {
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
    let Some(index) = world
        .resources
        .get::<TerrainCoverageIndex>()
        .filter(|_| style.terrain.is_some())
    else {
        return 0.0;
    };
    let position = camera.camera_position();
    let len = position.magnitude();
    if len > f64::EPSILON {
        let sin_lat = position.y / len;
        let sin_limit = crate::projection::globe::scale::MERCATOR_LATITUDE_LIMIT
            .to_radians()
            .sin();
        if sin_lat > sin_limit {
            return f64::from(index.poles().north_meters) * index.exaggeration();
        }
        if sin_lat < -sin_limit {
            return f64::from(index.poles().south_meters) * index.exaggeration();
        }
    }
    let nadir = crate::projection::globe::unit_sphere_to_lat_lon(position);
    let x = nadir.longitude / 360.0 + 0.5;
    let y = (1.0 - nadir.latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    index
        .loaded_ground_cached(&world.tiles, x, y, DemDetail::Finest)
        .unwrap_or(0.0)
}

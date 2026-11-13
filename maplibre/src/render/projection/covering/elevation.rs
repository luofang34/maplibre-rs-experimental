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

pub(crate) fn style_elevated_headroom_meters(style: &Style) -> f64 {
    let has_elevated_3d = style.layers.iter().any(|layer| match &layer.paint {
        Some(LayerPaint::FillExtrusion(paint)) => paint.fill_extrusion_translucent == Some(true),
        Some(LayerPaint::Line(paint)) => paint.line_elevation.is_some(),
        _ => false,
    });
    if has_elevated_3d {
        ELEVATED_3D_HEADROOM_METERS
    } else {
        0.0
    }
}

struct HeadroomTileElevation<P> {
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

/// Culling bounds per tile: the loaded DEM's range where terrain has one, else `fallback`,
/// widened upward by `headroom_meters` when the style renders elevated 3D volumes or lines.
pub(super) fn tile_elevation<'a>(
    style: &Style,
    world: &'a World,
    fallback: TileElevationRange,
    headroom_meters: f64,
) -> Box<dyn TileElevationProvider + 'a> {
    match world.resources.get::<TerrainCoverageIndex>() {
        Some(index) if style.terrain.is_some() => Box::new(HeadroomTileElevation {
            inner: IndexedTileElevation { index, fallback },
            headroom_meters,
        }),
        _ => Box::new(TileElevationRange {
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

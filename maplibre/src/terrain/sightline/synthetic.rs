//! Synthetic terrain for tests: rendered tiles and the DEM behind them, sampled from height
//! functions of Mercator position, around a Himalayan scene.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::Point2;
use image::{Rgba, RgbaImage};

use super::{lat_lon_to_mercator, DrawnTerrain};
use crate::{
    coords::{LatLon, WorldTileCoords, ZoomLevel},
    io::source_type::{RasterSource, SourceType},
    projection::{
        body::Body,
        globe::camera::{GlobeCameraOptions, GlobeCameraState},
    },
    style::source::TileAddressingScheme,
    tcs::tiles::Tiles,
    terrain::{
        coverage::TerrainCoverageIndex, dem::DemTile, source::DemSource, DemTileComponent,
        LoadedDem,
    },
};

pub(crate) const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];
pub(crate) const SCENE: LatLon = LatLon {
    latitude: 27.765_393_137_835_165,
    longitude: 88.054_643_590_047_59,
};
/// Zoom of the rendered tiles; their DEM is one zoom coarser.
pub(crate) const ZOOM: u8 = 11;
pub(crate) const DEM_ZOOM: u8 = ZOOM - 1;

pub(crate) fn source(maxzoom: u8) -> DemSource {
    exaggerated_source(maxzoom, 1.0)
}

/// A Terrarium DEM source up to `maxzoom`, drawn `exaggeration` times as high.
pub(crate) fn exaggerated_source(maxzoom: u8, exaggeration: f32) -> DemSource {
    DemSource {
        name: "dem".to_string(),
        source: SourceType::Raster(RasterSource::from_template(
            "https://dem.example/{z}/{x}/{y}.png",
            TileAddressingScheme::XYZ,
        )),
        tile_size: 256,
        unpack: TERRARIUM,
        minzoom: 0,
        maxzoom,
        exaggeration,
    }
}

pub(crate) fn terrarium(elevation: f64) -> Rgba<u8> {
    let value = ((elevation + 32768.0) * 256.0).round() as u32;
    Rgba([(value >> 16) as u8, (value >> 8) as u8, value as u8, 255])
}

/// A DEM tile whose samples read `height` at their Mercator position.
pub(crate) fn dem_tile(coords: WorldTileCoords, height: &impl Fn(Point2<f64>) -> f64) -> DemTile {
    let scale = 2_f64.powi(i32::from(u8::from(coords.z)));
    let image = RgbaImage::from_fn(256, 256, |x, y| {
        terrarium(height(Point2::new(
            (f64::from(coords.x) + (f64::from(x) + 0.5) / 256.0) / scale,
            (f64::from(coords.y) + (f64::from(y) + 0.5) / 256.0) / scale,
        )))
    });
    DemTile::from_image(&image, TERRARIUM).expect("tile decodes")
}

/// Tiles at `zoom` within `radius` tiles of `center`, wrapping across the antimeridian.
pub(crate) fn block(center: LatLon, zoom: u8, radius: i32) -> Vec<WorldTileCoords> {
    let scale = 2_i32.pow(u32::from(zoom));
    let mercator = lat_lon_to_mercator(center);
    let (cx, cy) = (
        (mercator.x * f64::from(scale)).floor() as i32,
        (mercator.y * f64::from(scale)).floor() as i32,
    );
    (-radius..=radius)
        .flat_map(|dy| (-radius..=radius).map(move |dx| (dx, dy)))
        .filter(|(_, dy)| (0..scale).contains(&(cy + dy)))
        .map(|(dx, dy)| WorldTileCoords {
            x: (cx + dx).rem_euclid(scale),
            y: cy + dy,
            z: ZoomLevel::new(zoom),
        })
        .collect()
}

/// Rendered tiles, the DEM loaded for them and the index over both.
pub(crate) struct Ground {
    pub(crate) tiles: Tiles,
    pub(crate) index: TerrainCoverageIndex,
}

impl Ground {
    /// `rendered` tiles drawing the DEM of `loaded` tiles, all sampling `height`.
    pub(crate) fn new(
        rendered: &[WorldTileCoords],
        loaded: &[WorldTileCoords],
        height: impl Fn(Point2<f64>) -> f64,
    ) -> Self {
        let mut tiles = Tiles::default();
        for coords in loaded {
            tiles
                .spawn_mut(*coords)
                .expect("valid coords")
                .insert(DemTileComponent::Loaded(LoadedDem::new(dem_tile(
                    *coords, &height,
                ))));
        }
        let index = TerrainCoverageIndex::build(rendered.iter().copied(), &tiles, &source(ZOOM));
        Self { tiles, index }
    }

    /// The same ground drawn `exaggeration` times as high.
    pub(crate) fn exaggerated(self, exaggeration: f32) -> Self {
        let rendered: Vec<WorldTileCoords> = self.index.rendered_tiles().collect();
        let index = TerrainCoverageIndex::build(
            rendered,
            &self.tiles,
            &exaggerated_source(ZOOM, exaggeration),
        );
        Self { index, ..self }
    }

    /// Tiles rendered around `center` with all their DEM loaded.
    pub(crate) fn around(center: LatLon, height: impl Fn(Point2<f64>) -> f64) -> Self {
        Self::new(&block(center, ZOOM, 4), &block(center, DEM_ZOOM, 3), height)
    }

    pub(crate) fn terrain(&self) -> DrawnTerrain<'_> {
        DrawnTerrain {
            index: &self.index,
            tiles: &self.tiles,
            body: Body::EARTH,
        }
    }
}

/// The 2330x1800 view of the vertical-perspective checks at zoom 11.67, orbiting `center`
/// raised to `elevation`.
pub(crate) fn camera(center: LatLon, pitch: f64, bearing: f64, elevation: f64) -> GlobeCameraState {
    GlobeCameraState::new(GlobeCameraOptions {
        width: 2330.0,
        height: 1800.0,
        field_of_view_degrees: 36.869_897_645_844_02,
        center,
        world_size: 512.0 * 2_f64.powf(11.67),
        bearing_degrees: bearing,
        pitch_degrees: pitch,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),
        body: Body::EARTH,
        target_elevation_meters: elevation,
    })
    .expect("camera")
}

/// Metres east and north of `origin`, on the sphere.
pub(crate) fn offset(origin: LatLon, east: f64, north: f64) -> LatLon {
    let radius = Body::EARTH.radius_meters;
    LatLon::new(
        origin.latitude + (north / radius).to_degrees(),
        origin.longitude + (east / (radius * origin.latitude.to_radians().cos())).to_degrees(),
    )
}

/// The location of Mercator coordinates in `0..1`.
pub(crate) fn location(mercator: Point2<f64>) -> LatLon {
    super::mercator_to_lat_lon(mercator)
}

/// Rolling hills around 1500 m, varying on a scale of a few kilometres.
pub(crate) fn hills(mercator: Point2<f64>) -> f64 {
    1500.0 + 600.0 * (mercator.x * 9000.0).sin() * (mercator.y * 7000.0).cos()
}

/// A ridge running north and south, `crest` metres high and about 500 m wide, along the
/// meridian `east` metres east of the scene, on level ground at sea level.
pub(crate) fn ridge(east: f64, crest: f64) -> impl Fn(Point2<f64>) -> f64 {
    let at = lat_lon_to_mercator(offset(SCENE, east, 0.0)).x;
    let half_width =
        250.0 / (Body::EARTH.circumference_meters() * SCENE.latitude.to_radians().cos());
    move |mercator| {
        if (mercator.x - at).abs() < half_width {
            crest
        } else {
            0.0
        }
    }
}

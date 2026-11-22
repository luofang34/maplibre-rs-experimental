//! Geographic coordinates (`LatLon`) and Mercator pixel coordinates (`WorldCoords`).

use std::{
    f64::consts::PI,
    fmt,
    fmt::{Display, Formatter},
};

use cgmath::{Point2, Point3};

use crate::{WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE};

#[derive(Copy, Clone, Debug, PartialEq)]
/// Geographic coordinates in degrees, ordered latitude before longitude.
pub struct LatLon {
    /// Latitude in degrees north of the equator; no range is enforced by this type.
    pub latitude: f64,
    /// Longitude in degrees east of the prime meridian; no wrapping is performed by this type.
    pub longitude: f64,
}

impl LatLon {
    /// Stores coordinates in degrees without clamping latitude or wrapping longitude.
    pub fn new(latitude: f64, longitude: f64) -> Self {
        LatLon {
            latitude,
            longitude,
        }
    }

    /// Projects this geographic location to normalized Mercator coordinates in `[0..1] x [0..1]`.
    pub fn to_mercator(&self) -> Point2<f64> {
        let x = self.longitude / 360.0 + 0.5;
        let y = (1.0 - self.latitude.to_radians().tan().asinh() / PI) * 0.5;
        Point2::new(x, y.clamp(0.0, 1.0 - 1e-9))
    }

    /// Converts normalized Mercator coordinates in `[0..1] x [0..1]` back to geographic coordinates.
    pub fn from_mercator(mercator: Point2<f64>) -> Self {
        let lat = (PI * (1.0 - 2.0 * mercator.y)).sinh().atan().to_degrees();
        let lon = mercator.x * 360.0 - 180.0;
        LatLon::new(lat, lon)
    }
}

impl Default for LatLon {
    fn default() -> Self {
        LatLon {
            latitude: 0.0,
            longitude: 0.0,
        }
    }
}

impl Display for LatLon {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.latitude, self.longitude)
    }
}

/// Mercator coordinates measured in world pixels at a particular zoom.
///
/// # Coordinate System Origin
///
/// The origin of the coordinate system is in the upper-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldCoords {
    /// Pixel distance east from the Mercator world's western edge at the associated zoom.
    pub x: f64,
    /// Pixel distance south from the Mercator world's northern edge at the associated zoom.
    pub y: f64,
}

impl WorldCoords {
    /// Projects degrees to Mercator pixels at `zoom` without clamping latitude or wrapping longitude.
    /// Callers must supply finite longitude and latitude strictly between -90 and 90 degrees.
    pub fn from_lat_lon(lat_lon: LatLon, zoom: Zoom) -> WorldCoords {
        let tile_size = TILE_SIZE * 2.0_f64.powf(zoom.value());
        let x = (lat_lon.longitude + 180.0) * (tile_size / 360.0);
        let lat_rad = (lat_lon.latitude * PI) / 180.0;
        let merc_n = f64::ln(f64::tan((PI / 4.0) + (lat_rad / 2.0)));
        let y = (tile_size / 2.0) - (tile_size * merc_n / (2.0 * PI));
        WorldCoords { x, y }
    }

    /// Stores a ground-plane pixel position; the associated zoom remains the caller's responsibility.
    pub fn at_ground(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Converts pixels at `zoom` to tile indices at `z`, flooring fractional indices, so a point
    /// left of the world lies in tile `-1`. The returned indices are not checked against
    /// canonical tile bounds.
    pub fn into_world_tile(self, z: ZoomLevel, zoom: Zoom) -> WorldTileCoords {
        let tile_scale = zoom.scale_to_zoom_level(z) / TILE_SIZE;
        let x = self.x * tile_scale;
        let y = self.y * tile_scale;

        WorldTileCoords {
            x: x.floor() as i32,
            y: y.floor() as i32,
            z,
        }
    }
}

impl From<(f32, f32)> for WorldCoords {
    fn from(tuple: (f32, f32)) -> Self {
        WorldCoords {
            x: tuple.0 as f64,
            y: tuple.1 as f64,
        }
    }
}

impl From<(f64, f64)> for WorldCoords {
    fn from(tuple: (f64, f64)) -> Self {
        WorldCoords {
            x: tuple.0,
            y: tuple.1,
        }
    }
}

impl From<Point3<f64>> for WorldCoords {
    fn from(point: Point3<f64>) -> Self {
        WorldCoords {
            x: point.x,
            y: point.y,
        }
    }
}

impl From<WorldCoords> for [f32; 3] {
    fn from(world_coords: WorldCoords) -> Self {
        [world_coords.x as f32, world_coords.y as f32, 0.0]
    }
}

impl Display for WorldCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "W(x={x},y={y})", x = self.x, y = self.y)
    }
}

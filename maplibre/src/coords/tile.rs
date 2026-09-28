//! Canonical and wrapped tile coordinates and their map-space transforms.
use super::*;

/// Within each tile there is a separate coordinate system. Usually this coordinate system is
/// within [`EXTENT`]. Therefore, `x` and `y` must be within the bounds of [`EXTENT`].
///
/// # Coordinate System Origin
///
/// The origin is in the upper-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InnerCoords {
    pub x: f64,
    pub y: f64,
}

/// Every tile has tile coordinates. These tile coordinates are also called
/// [Slippy map tile names](https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames).
///
/// # Coordinate System Origin
///
/// For Web Mercator the origin of the coordinate system is in the upper-left corner.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Default)]
pub struct TileCoords {
    pub x: u32,
    pub y: u32,
    pub z: ZoomLevel,
}

impl TileCoords {
    /// Transforms the tile coordinates as defined by the tile grid addressing scheme into a
    /// representation which is used in the 3d-world.
    /// This is not possible if the coordinates of this [`TileCoords`] exceed their bounds.
    ///
    /// # Example
    /// The [`TileCoords`] `T(x=5,y=5,z=0)` exceeds its bounds because there is no tile
    /// `x=5,y=5` at zoom level `z=0`.
    pub fn into_world_tile(self, scheme: TileAddressingScheme) -> Option<WorldTileCoords> {
        // FIXME: MAX_ZOOM is 32, which means max bound is 2^32, which wouldn't fit in u32 or i32
        // Note that unlike WorldTileCoords, values are signed (no idea why)
        let bounds = ZOOM_BOUNDS[self.z.0 as usize] as i32;
        let x = self.x as i32;
        let y = self.y as i32;

        if x >= bounds || y >= bounds {
            return None;
        }

        Some(match scheme {
            TileAddressingScheme::XYZ => WorldTileCoords { x, y, z: self.z },
            TileAddressingScheme::TMS => WorldTileCoords {
                x,
                y: bounds - 1 - y,
                z: self.z,
            },
        })
    }
}

impl From<(u32, u32, ZoomLevel)> for TileCoords {
    fn from(tuple: (u32, u32, ZoomLevel)) -> Self {
        TileCoords {
            x: tuple.0,
            y: tuple.1,
            z: tuple.2,
        }
    }
}

/// Every tile has tile coordinates. Every tile coordinate can be mapped to a coordinate within
/// the world. This provides the freedom to map from [TMS](https://wiki.openstreetmap.org/wiki/TMS)
/// to [Slippy map tile names](https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames).
///
/// # Coordinate System Origin
///
/// The origin of the coordinate system is in the upper-left corner.
// FIXME: does Zeroable make sense?
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    Zeroable,
)]
#[repr(C)]
pub struct WorldTileCoords {
    pub x: i32,
    pub y: i32,
    pub z: ZoomLevel,
}

impl WorldTileCoords {
    /// Returns the tile coords according to an addressing scheme. This is not possible if the
    /// coordinates of this [`WorldTileCoords`] exceed their bounds.
    ///
    /// # Example
    ///
    /// The [`WorldTileCoords`] `WT(x=5,y=5,z=0)` exceeds its bounds because there is no tile
    /// `x=5,y=5` at zoom level `z=0`.
    pub fn into_tile(self, scheme: TileAddressingScheme) -> Option<TileCoords> {
        // FIXME: MAX_ZOOM is 32, which means max bound is 2^32, which wouldn't fit in u32 or i32
        let bounds = ZOOM_BOUNDS[self.z.0 as usize];
        let x = self.x as u32;
        let y = self.y as u32;

        if x >= bounds || y >= bounds {
            return None;
        }

        Some(match scheme {
            TileAddressingScheme::XYZ => TileCoords { x, y, z: self.z },
            TileAddressingScheme::TMS => TileCoords {
                x,
                y: bounds - 1 - y,
                z: self.z,
            },
        })
    }

    /// Adopted from
    /// [Transform::calculatePosMatrix](https://github.com/maplibre/maplibre-gl-js/blob/80e232a64716779bfff841dbc18fddc1f51535ad/src/geo/transform.ts#L719-L731)
    #[tracing::instrument(skip_all)]
    pub fn transform_for_zoom(&self, zoom: Zoom) -> Matrix4<f64> {
        /*
           For tile.z = zoom:
               => scale = 512
           If tile.z < zoom:
               => scale > 512
           If tile.z > zoom:
               => scale < 512
        */
        let tile_scale = TILE_SIZE * Zoom::from(self.z).scale_delta(&zoom);

        let translate = Matrix4::from_translation(Vector3::new(
            self.x as f64 * tile_scale,
            self.y as f64 * tile_scale,
            0.0,
        ));

        // Divide by EXTENT to normalize tile
        // Scale tiles where zoom level = self.z to 512x512
        let normalize_and_scale =
            Matrix4::from_nonuniform_scale(tile_scale / EXTENT, tile_scale / EXTENT, 1.0);
        translate * normalize_and_scale
    }

    pub fn into_aligned(self) -> AlignedWorldTileCoords {
        AlignedWorldTileCoords(WorldTileCoords {
            x: div_floor(self.x, 2) * 2,
            y: div_floor(self.y, 2) * 2,
            z: self.z,
        })
    }

    /// Adopted from [tilebelt](https://github.com/mapbox/tilebelt)
    pub fn build_quad_key(&self) -> Option<Quadkey> {
        let bounds = ZOOM_BOUNDS[self.z.0 as usize];
        let x = self.x as u32;
        let y = self.y as u32;

        if x >= bounds || y >= bounds {
            return None;
        }

        let mut key = [ZoomLevel::default(); MAX_ZOOM];

        key[0] = self.z;

        for z in 1..self.z.0 + 1 {
            let mut b = 0;
            let mask: i32 = 1 << (z - 1);
            if (self.x & mask) != 0 {
                b += 1u8;
            }
            if (self.y & mask) != 0 {
                b += 2u8;
            }
            key[z as usize] = ZoomLevel::from(b);
        }
        Some(Quadkey(key))
    }

    /// Adopted from [tilebelt](https://github.com/mapbox/tilebelt)
    pub fn get_children(&self) -> [WorldTileCoords; 4] {
        [
            WorldTileCoords {
                x: self.x * 2,
                y: self.y * 2,
                z: self.z + 1,
            },
            WorldTileCoords {
                x: self.x * 2 + 1,
                y: self.y * 2,
                z: self.z + 1,
            },
            WorldTileCoords {
                x: self.x * 2 + 1,
                y: self.y * 2 + 1,
                z: self.z + 1,
            },
            WorldTileCoords {
                x: self.x * 2,
                y: self.y * 2 + 1,
                z: self.z + 1,
            },
        ]
    }

    /// Get the tile which is one zoom level lower and contains this one
    pub fn get_parent(&self) -> Option<WorldTileCoords> {
        if self.z.is_root() {
            return None;
        }

        Some(WorldTileCoords {
            x: self.x >> 1,
            y: self.y >> 1,
            z: self.z - 1,
        })
    }

    /// Returns unique stencil reference values for WorldTileCoords which are 3D.
    /// Tiles from arbitrary `z` can lie next to each other, because we mix tiles from
    /// different levels based on availability.
    pub fn stencil_reference_value_3d(&self) -> u8 {
        const CASES: u8 = 4;
        let z = u8::from(self.z);
        match (self.x % 2 == 0, self.y % 2 == 0) {
            (true, true) => z * CASES,
            (true, false) => 1 + z * CASES,
            (false, true) => 2 + z * CASES,
            (false, false) => 3 + z * CASES,
        }
    }
}

impl From<(i32, i32, ZoomLevel)> for WorldTileCoords {
    fn from(tuple: (i32, i32, ZoomLevel)) -> Self {
        WorldTileCoords {
            x: tuple.0,
            y: tuple.1,
            z: tuple.2,
        }
    }
}

/// An aligned world tile coordinate aligns a world coordinate at a 4x4 tile raster within the
/// world. The aligned coordinates is defined by the coordinates of the upper left tile in the 4x4
/// tile raster divided by 2 and rounding to the ceiling.
///
///
/// # Coordinate System Origin
///
/// The origin of the coordinate system is in the upper-left corner.
pub struct AlignedWorldTileCoords(pub WorldTileCoords);

impl AlignedWorldTileCoords {
    pub fn upper_left(self) -> WorldTileCoords {
        self.0
    }

    pub fn upper_right(&self) -> WorldTileCoords {
        WorldTileCoords {
            x: self.0.x + 1,
            y: self.0.y,
            z: self.0.z,
        }
    }

    pub fn lower_left(&self) -> WorldTileCoords {
        WorldTileCoords {
            x: self.0.x,
            y: self.0.y - 1,
            z: self.0.z,
        }
    }

    pub fn lower_right(&self) -> WorldTileCoords {
        WorldTileCoords {
            x: self.0.x + 1,
            y: self.0.y + 1,
            z: self.0.z,
        }
    }
}

impl Display for TileCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "T(x={x},y={y},z={z})",
            x = self.x,
            y = self.y,
            z = self.z
        )
    }
}

impl Display for WorldTileCoords {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WT(x={x},y={y},z={z})",
            x = self.x,
            y = self.y,
            z = self.z
        )
    }
}

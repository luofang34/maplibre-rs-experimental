//! Canonical and wrapped tile coordinates and their map-space transforms.
use super::*;

/// Tile-local coordinates, where [`EXTENT`] spans one tile edge.
/// Buffered geometry may extend beyond the tile boundary.
///
/// # Coordinate System Origin
///
/// The origin is in the upper-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InnerCoords {
    /// Horizontal distance from the tile's left edge in tile-local units.
    pub x: f64,
    /// Vertical distance from the tile's top edge in tile-local units.
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
    /// Column index, increasing eastward; construction does not validate the grid bounds.
    pub x: u32,
    /// Row index; its origin is selected by [`TileAddressingScheme`] during conversion.
    pub y: u32,
    /// Discrete tile-grid level.
    pub z: ZoomLevel,
}

impl TileCoords {
    /// Transforms the tile coordinates as defined by the tile grid addressing scheme into a
    /// representation which is used in the 3d-world.
    /// Returns `None` if the level is unsupported or either index is outside `0..2^z`.
    ///
    /// # Example
    /// The [`TileCoords`] `T(x=5,y=5,z=0)` exceeds its bounds because there is no tile
    /// `x=5,y=5` at zoom level `z=0`.
    pub fn into_world_tile(self, scheme: TileAddressingScheme) -> Option<WorldTileCoords> {
        let bounds = *ZOOM_BOUNDS.get(self.z.0 as usize)?;
        if self.x >= bounds || self.y >= bounds {
            return None;
        }
        // Canonical indices fit i32 even at z=31; the exclusive bound does not.
        let x = self.x as i32;
        let y = match scheme {
            TileAddressingScheme::XYZ => self.y,
            TileAddressingScheme::TMS => bounds - 1 - self.y,
        } as i32;
        Some(WorldTileCoords { x, y, z: self.z })
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
    /// Column index increasing eastward; negative values can represent wrapped worlds.
    pub x: i32,
    /// Row index increasing southward from the northern Mercator boundary.
    pub y: i32,
    /// Discrete tile-grid level; construction does not validate its range.
    pub z: ZoomLevel,
}

impl WorldTileCoords {
    /// Returns the tile coords according to an addressing scheme. This is not possible if the
    /// level is unsupported or either index lies outside `0..2^z`; wrapped tiles are rejected.
    ///
    /// # Example
    ///
    /// The [`WorldTileCoords`] `WT(x=5,y=5,z=0)` exceeds its bounds because there is no tile
    /// `x=5,y=5` at zoom level `z=0`.
    pub fn into_tile(self, scheme: TileAddressingScheme) -> Option<TileCoords> {
        let bounds = *ZOOM_BOUNDS.get(self.z.0 as usize)?;
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

    /// Maps tile-local x/y units to world pixels at `zoom`, leaving the z coordinate unchanged.
    /// Based on
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

    /// Floors both indices to the even-indexed anchor of a containing 2-by-2 tile block.
    pub fn into_aligned(self) -> AlignedWorldTileCoords {
        AlignedWorldTileCoords(WorldTileCoords {
            x: div_floor(self.x, 2) * 2,
            y: div_floor(self.y, 2) * 2,
            z: self.z,
        })
    }

    /// The tile of the world this coordinate repeats and how many worlds away it lies, for a
    /// tile that a view sees in a copy of the world; `None` when the row or level is outside
    /// the grid, which no copy can fix.
    pub fn wrapped(self) -> Option<(WorldTileCoords, i32)> {
        let bounds = i64::from(*ZOOM_BOUNDS.get(self.z.0 as usize)?);
        if !(0..bounds).contains(&i64::from(self.y)) {
            return None;
        }
        let x = i64::from(self.x);
        let canonical = WorldTileCoords {
            x: x.rem_euclid(bounds) as i32,
            y: self.y,
            z: self.z,
        };
        Some((canonical, x.div_euclid(bounds) as i32))
    }

    /// Builds a key from least-significant coordinate bits first, or `None` outside the grid.
    /// Quadrant encoding follows [tilebelt](https://github.com/mapbox/tilebelt).
    pub fn build_quad_key(&self) -> Option<Quadkey> {
        let bounds = *ZOOM_BOUNDS.get(self.z.0 as usize)?;
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

    /// Returns children clockwise from the upper left, without clipping to canonical bounds.
    /// Callers must leave room in the signed indices and zoom level for doubling and incrementing.
    /// Child ordering follows [tilebelt](https://github.com/mapbox/tilebelt).
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

/// Upper-left anchor of a 2-by-2 tile block with x eastward and y southward.
/// [`WorldTileCoords::into_aligned`] supplies even indices; direct construction is unchecked.
pub struct AlignedWorldTileCoords(
    /// Anchor at the block's tile-grid level, with space for a neighbor at each index plus one.
    pub WorldTileCoords,
);

impl AlignedWorldTileCoords {
    /// Returns the stored upper-left anchor.
    pub fn upper_left(self) -> WorldTileCoords {
        self.0
    }

    /// Returns the tile immediately east of the anchor.
    pub fn upper_right(&self) -> WorldTileCoords {
        WorldTileCoords {
            x: self.0.x + 1,
            y: self.0.y,
            z: self.0.z,
        }
    }

    /// Returns the tile immediately south of the anchor.
    pub fn lower_left(&self) -> WorldTileCoords {
        WorldTileCoords {
            x: self.0.x,
            y: self.0.y + 1,
            z: self.0.z,
        }
    }

    /// Returns the tile one column east and one row south of the anchor.
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

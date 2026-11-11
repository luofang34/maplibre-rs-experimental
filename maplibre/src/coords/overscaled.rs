//! Source-clamped overscaled tile identifiers, modeled after `maplibre-native`
//! `mbgl::OverscaledTileID` and `maplibre-gl-js` `OverscaledTileID`.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::coords::{WorldTileCoords, ZoomLevel, MAX_ZOOM, ZOOM_BOUNDS};

/// Tile identifier combining canonical source coordinates, horizontal world wrap, and
/// effective overscaled zoom (`overscaled_z >= canonical.z`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OverscaledTileId {
    /// Effective zoom level at which this tile is evaluated or displayed (`>= canonical.z`).
    pub overscaled_z: u8,
    /// Horizontal world wrap index (`0` for the primary world `[-180°, +180°)`).
    pub wrap: i16,
    /// Canonical source tile coordinates (`0 <= x, y < 2^z`).
    pub canonical: WorldTileCoords,
}

impl OverscaledTileId {
    /// Constructs an overscaled tile identifier when `canonical` lies inside the canonical grid
    /// and `overscaled_z` is at least `canonical.z` and below [`MAX_ZOOM`].
    pub fn new(overscaled_z: u8, wrap: i16, canonical: WorldTileCoords) -> Option<Self> {
        let canonical_z = u8::from(canonical.z);
        if usize::from(overscaled_z) >= MAX_ZOOM || overscaled_z < canonical_z {
            return None;
        }
        let bounds = *ZOOM_BOUNDS.get(usize::from(canonical_z))?;
        if canonical.x < 0
            || canonical.y < 0
            || (canonical.x as u32) >= bounds
            || (canonical.y as u32) >= bounds
        {
            return None;
        }
        Some(Self {
            overscaled_z,
            wrap,
            canonical,
        })
    }

    /// Wraps `coords` into canonical coordinates and world `wrap` at its native zoom level.
    pub fn from_wrapped(coords: WorldTileCoords) -> Option<Self> {
        let (canonical, wrap) = coords.wrapped()?;
        let wrap = i16::try_from(wrap).ok()?;
        Some(Self {
            overscaled_z: u8::from(canonical.z),
            wrap,
            canonical,
        })
    }

    /// Resolves a view tile against a source's `max_zoom`, clamping `canonical` to `max_zoom`
    /// while recording `overscaled_z` at `target_z` when `retain_overscale` is set.
    pub fn for_source(
        view_coords: WorldTileCoords,
        target_z: u8,
        source_max_zoom: Option<u8>,
        retain_overscale: bool,
    ) -> Option<Self> {
        let (mut canonical, wrap) = view_coords.wrapped()?;
        let wrap = i16::try_from(wrap).ok()?;
        if let Some(max_z) = source_max_zoom {
            let current_z = u8::from(canonical.z);
            if current_z > max_z {
                let shift = u32::from(current_z - max_z);
                canonical = WorldTileCoords {
                    x: canonical.x >> shift,
                    y: canonical.y >> shift,
                    z: ZoomLevel::new(max_z),
                };
            }
        }
        let canonical_z = u8::from(canonical.z);
        let overscaled_z = if retain_overscale && source_max_zoom.is_some_and(|m| canonical_z >= m)
        {
            target_z.max(canonical_z).min((MAX_ZOOM - 1) as u8)
        } else {
            canonical_z
        };
        Some(Self {
            overscaled_z,
            wrap,
            canonical,
        })
    }

    /// Integer magnification factor `2^(overscaled_z - canonical.z)`.
    #[inline]
    pub const fn overscale_factor(&self) -> u32 {
        let diff = self.overscaled_z.saturating_sub(self.canonical.z.0);
        1_u32 << (diff as u32)
    }

    /// Whether this tile is magnified past its canonical source zoom.
    #[inline]
    pub const fn is_overscaled(&self) -> bool {
        self.overscaled_z > self.canonical.z.0
    }

    /// Layout/evaluation zoom passed to workers: `overscaled_z` when magnified, `0` otherwise.
    #[inline]
    pub const fn worker_overscaled_zoom(&self) -> u8 {
        if self.is_overscaled() {
            self.overscaled_z
        } else {
            0
        }
    }

    /// Rescales this tile identifier to `target_z`, walking `canonical` to an ancestor when
    /// `target_z < canonical.z`, or adjusting `overscaled_z` when `target_z >= canonical.z`.
    pub fn scaled_to(&self, target_z: u8) -> Option<Self> {
        if usize::from(target_z) >= MAX_ZOOM {
            return None;
        }
        let canonical_z = u8::from(self.canonical.z);
        if target_z >= canonical_z {
            Some(Self {
                overscaled_z: target_z,
                wrap: self.wrap,
                canonical: self.canonical,
            })
        } else {
            let shift = u32::from(canonical_z - target_z);
            Some(Self {
                overscaled_z: target_z,
                wrap: self.wrap,
                canonical: WorldTileCoords {
                    x: self.canonical.x >> shift,
                    y: self.canonical.y >> shift,
                    z: ZoomLevel::new(target_z),
                },
            })
        }
    }

    /// Whether `self` lies in the same wrapped world and strictly deeper in `parent`'s subtree.
    pub fn is_child_of(&self, parent: &Self) -> bool {
        if self.wrap != parent.wrap || self.overscaled_z <= parent.overscaled_z {
            return false;
        }
        let self_cz = u8::from(self.canonical.z);
        let parent_cz = u8::from(parent.canonical.z);
        if self_cz < parent_cz {
            return false;
        }
        let shift = u32::from(self_cz - parent_cz);
        (self.canonical.x >> shift) == parent.canonical.x
            && (self.canonical.y >> shift) == parent.canonical.y
    }

    /// Direct children at `overscaled_z + 1`: four subdivided canonical tiles below
    /// `source_max_zoom`, or one virtual overscaled child once `canonical` reaches `source_max_zoom`.
    pub fn children(&self, source_max_zoom: u8) -> OverscaledChildren {
        let next_z = self.overscaled_z.saturating_add(1);
        if usize::from(next_z) >= MAX_ZOOM {
            return OverscaledChildren {
                items: [*self; 4],
                len: 0,
                index: 0,
            };
        }
        if self.overscaled_z >= source_max_zoom {
            let virtual_child = Self {
                overscaled_z: next_z,
                wrap: self.wrap,
                canonical: self.canonical,
            };
            OverscaledChildren {
                items: [virtual_child; 4],
                len: 1,
                index: 0,
            }
        } else {
            let sub = self.canonical.get_children().map(|child| Self {
                overscaled_z: next_z,
                wrap: self.wrap,
                canonical: child,
            });
            OverscaledChildren {
                items: sub,
                len: 4,
                index: 0,
            }
        }
    }

    /// Placed `WorldTileCoords` shifted by `wrap` worlds at `canonical.z`.
    pub fn to_wrapped_coords(&self) -> WorldTileCoords {
        let span = 1_i64 << u32::from(u8::from(self.canonical.z));
        WorldTileCoords {
            x: (i64::from(self.canonical.x) + i64::from(self.wrap) * span) as i32,
            y: self.canonical.y,
            z: self.canonical.z,
        }
    }
}

impl Display for OverscaledTileId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OT(oz={},wrap={},x={},y={},z={})",
            self.overscaled_z, self.wrap, self.canonical.x, self.canonical.y, self.canonical.z
        )
    }
}

/// Stack-allocated iterator over the `0`, `1`, or `4` children of an [`OverscaledTileId`].
#[derive(Clone, Copy, Debug)]
pub struct OverscaledChildren {
    items: [OverscaledTileId; 4],
    len: u8,
    index: u8,
}

impl Iterator for OverscaledChildren {
    type Item = OverscaledTileId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.len {
            return None;
        }
        let item = self.items[usize::from(self.index)];
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = usize::from(self.len.saturating_sub(self.index));
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for OverscaledChildren {}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(x: i32, y: i32, z: u8) -> WorldTileCoords {
        WorldTileCoords {
            x,
            y,
            z: ZoomLevel::new(z),
        }
    }

    #[test]
    fn source_clamping_and_overscale_factor_match_gl_js_semantics() {
        let id = OverscaledTileId::for_source(wt(-1, 5, 4), 4, Some(2), true).unwrap();
        assert_eq!(id.wrap, -1);
        assert_eq!(id.canonical, wt(3, 1, 2));
        assert_eq!(id.overscaled_z, 4);
        assert_eq!(id.overscale_factor(), 4);
        assert_eq!(id.worker_overscaled_zoom(), 4);

        let unretained = OverscaledTileId::for_source(wt(15, 5, 4), 4, Some(2), false).unwrap();
        assert_eq!(unretained.wrap, 0);
        assert_eq!(unretained.canonical, wt(3, 1, 2));
        assert_eq!(unretained.overscaled_z, 2);
        assert_eq!(unretained.overscale_factor(), 1);
        assert_eq!(unretained.worker_overscaled_zoom(), 0);
    }

    #[test]
    fn children_subdivide_below_max_zoom_and_overscale_at_max_zoom() {
        let base = OverscaledTileId::new(2, 0, wt(1, 2, 2)).unwrap();
        let sub: Vec<_> = base.children(3).collect();
        assert_eq!(sub.len(), 4);
        assert!(sub.iter().all(|child| child.is_child_of(&base)));

        let capped: Vec<_> = base.children(2).collect();
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].canonical, wt(1, 2, 2));
        assert_eq!(capped[0].overscaled_z, 3);
        assert!(capped[0].is_child_of(&base));
        assert_eq!(capped[0].scaled_to(2), Some(base));
        assert_eq!(
            capped[0].scaled_to(1),
            OverscaledTileId::new(1, 0, wt(0, 1, 1))
        );
    }
}

//! Vertical reference datums and shell part selectors for 3D extrusions and elevated lines.

use super::{FillExtrusionPaint, LinePaint, StyleProperty};

/// Subset of an extruded prism's shell emitted by a `fill-extrusion` feature, allowing multi-tier
/// 3D volumes (such as stepped Class B/C airspace shelves) to be sliced into watertight manifold
/// exteriors without internal coplanar walls or duplicate horizontal caps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExtrusionPart {
    /// Complete prism: roof cap, walls, and (when translucent with `base > 0`) bottom floor cap.
    #[default]
    Full,
    /// Vertical perimeter and hole walls only, with no roof or floor cap.
    Walls,
    /// Top roof cap at `fill-extrusion-height` only (`+Up` outward normal).
    Roof,
    /// Bottom floor cap at `fill-extrusion-base` only (`-Up` outward normal).
    Floor,
}

impl ExtrusionPart {
    /// Parses a `fill-extrusion-part` string; unrecognized values default to [`Self::Full`].
    pub fn from_extrusion_str(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("walls") || trimmed.eq_ignore_ascii_case("wall") {
            Self::Walls
        } else if trimmed.eq_ignore_ascii_case("roof")
            || trimmed.eq_ignore_ascii_case("top")
            || trimmed.eq_ignore_ascii_case("ceiling")
        {
            Self::Roof
        } else if trimmed.eq_ignore_ascii_case("floor")
            || trimmed.eq_ignore_ascii_case("bottom")
            || trimmed.eq_ignore_ascii_case("base")
        {
            Self::Floor
        } else {
            Self::Full
        }
    }
}

/// Vertical reference surface from which a height, base or line elevation is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ElevationDatum {
    /// Measured above the terrain sample at the middle of the polygon's footprint, keeping
    /// buildings level across sloped ground.
    #[default]
    CentroidAgl,
    /// Measured above mean sea level (ellipsoid/orthometric zero), unaffected by terrain height.
    Msl,
    /// Measured above the terrain surface under each individual vertex (`agl` / `sfc`).
    VertexAgl,
}

impl ElevationDatum {
    /// Parses a `fill-extrusion-base-datum` or `fill-extrusion-height-datum` value; defaults to
    /// [`Self::CentroidAgl`] for standard building behavior.
    pub fn from_extrusion_str(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("msl")
            || trimmed.eq_ignore_ascii_case("amsl")
            || trimmed.eq_ignore_ascii_case("sea")
        {
            Self::Msl
        } else if trimmed.eq_ignore_ascii_case("agl")
            || trimmed.eq_ignore_ascii_case("sfc")
            || trimmed.eq_ignore_ascii_case("surface")
            || trimmed.eq_ignore_ascii_case("terrain")
        {
            Self::VertexAgl
        } else {
            Self::CentroidAgl
        }
    }

    /// Parses a `line-elevation-datum` value; defaults to [`Self::Msl`] when `line-elevation` is
    /// present so elevated contour and airspace rims sit at fixed altitude unless `"agl"` is set.
    pub fn from_line_str(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("agl")
            || trimmed.eq_ignore_ascii_case("sfc")
            || trimmed.eq_ignore_ascii_case("surface")
            || trimmed.eq_ignore_ascii_case("terrain")
        {
            Self::VertexAgl
        } else if trimmed.eq_ignore_ascii_case("centroid-agl")
            || trimmed.eq_ignore_ascii_case("footprint")
        {
            Self::CentroidAgl
        } else {
            Self::Msl
        }
    }

    /// Compact 0..=2 code where `0` is [`Self::CentroidAgl`].
    pub const fn code(self) -> u8 {
        match self {
            Self::CentroidAgl => 0,
            Self::Msl => 1,
            Self::VertexAgl => 2,
        }
    }

    /// Restores a datum from [`Self::code`].
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Msl,
            2 => Self::VertexAgl,
            _ => Self::CentroidAgl,
        }
    }

    /// Packs `(base_datum, height_datum)` into `0..=8`, with `(CentroidAgl, CentroidAgl) == 0`.
    pub const fn pack_pair(base: Self, height: Self) -> u8 {
        base.code() * 3 + height.code()
    }

    /// Unpacks [`Self::pack_pair`] back into `(base_datum, height_datum)`.
    pub const fn unpack_pair(pair: u8) -> (Self, Self) {
        (Self::from_code(pair / 3), Self::from_code(pair % 3))
    }
}

fn property_may_be_vertex_agl(
    property: Option<&StyleProperty<String>>,
    parse: fn(&str) -> ElevationDatum,
) -> bool {
    match property {
        None | Some(StyleProperty::Unsupported(_)) => false,
        Some(StyleProperty::Constant(value)) => parse(value) == ElevationDatum::VertexAgl,
        Some(StyleProperty::Expression(_)) => true,
    }
}

impl FillExtrusionPaint {
    /// Whether any feature in this extrusion layer may evaluate its base or height datum to
    /// [`ElevationDatum::VertexAgl`], requiring per-tile mesh subdivision over sloped terrain.
    pub fn may_use_vertex_agl(&self) -> bool {
        property_may_be_vertex_agl(
            self.fill_extrusion_base_datum.as_ref(),
            ElevationDatum::from_extrusion_str,
        ) || property_may_be_vertex_agl(
            self.fill_extrusion_height_datum.as_ref(),
            ElevationDatum::from_extrusion_str,
        )
    }
}

impl LinePaint {
    /// Whether any feature in this elevated line layer may evaluate its datum to
    /// [`ElevationDatum::VertexAgl`], requiring segment subdivision over sloped terrain.
    pub fn may_use_vertex_agl(&self) -> bool {
        self.line_elevation.is_some()
            && property_may_be_vertex_agl(
                self.line_elevation_datum.as_ref(),
                ElevationDatum::from_line_str,
            )
    }
}

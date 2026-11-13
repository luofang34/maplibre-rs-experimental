//! Offline geographic region definitions and deterministic Web Mercator tile-pyramid enumeration.

use std::{collections::BTreeSet, f64::consts::PI};

use serde::{Deserialize, Serialize};

pub use super::enumerate::{
    enumerate_style_resources, pending_non_volatile_tile_json_urls, OfflineResourceDescriptor,
};
use crate::coords::{WorldTileCoords, ZoomLevel};

const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_6;
const MAX_ENUMERABLE_ZOOM: u8 = 22;

/// Geographic bounding box, zoom range, display scale, and style reference defining an offline pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfflineRegionDefinition {
    /// Remote or local style JSON URL, when the region is backed by an external style document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_url: Option<String>,
    /// Inline MapLibre style JSON string, when supplied directly without a style URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_json: Option<String>,
    /// Geographic bounding box `[west, south, east, north]` in degrees (`west > east` crosses the antimeridian).
    pub bounds: [f64; 4],
    /// Minimum discrete tile zoom level to download (`0..=22`).
    pub min_zoom: u8,
    /// Maximum discrete tile zoom level to download (`min_zoom..=22`).
    pub max_zoom: u8,
    /// Display device pixel ratio (e.g. `1.0` or `2.0`).
    pub pixel_ratio: f32,
    /// Opaque application-defined metadata bytes persisted alongside the region.
    #[serde(default)]
    pub metadata: Vec<u8>,
}

impl OfflineRegionDefinition {
    /// Creates a region definition referencing a `style_url` across `bounds` (`[west, south, east, north]`).
    pub fn from_style_url(
        style_url: impl Into<String>,
        bounds: [f64; 4],
        min_zoom: u8,
        max_zoom: u8,
    ) -> Self {
        Self {
            style_url: Some(style_url.into()),
            style_json: None,
            bounds,
            min_zoom,
            max_zoom,
            pixel_ratio: 1.0,
            metadata: Vec::new(),
        }
    }

    /// Creates a region definition from an inline `style_json` document across `bounds`.
    pub fn from_style_json(
        style_json: impl Into<String>,
        bounds: [f64; 4],
        min_zoom: u8,
        max_zoom: u8,
    ) -> Self {
        Self {
            style_url: None,
            style_json: Some(style_json.into()),
            bounds,
            min_zoom,
            max_zoom,
            pixel_ratio: 1.0,
            metadata: Vec::new(),
        }
    }

    /// Sets the display `pixel_ratio` for the offline region.
    pub fn with_pixel_ratio(mut self, pixel_ratio: f32) -> Self {
        self.pixel_ratio = pixel_ratio;
        self
    }

    /// Attaches opaque application `metadata` bytes to the region.
    pub fn with_metadata(mut self, metadata: impl Into<Vec<u8>>) -> Self {
        self.metadata = metadata.into();
        self
    }

    /// Deterministically enumerates all `WorldTileCoords` intersecting `self.bounds` across
    /// `self.min_zoom..=self.max_zoom` in ascending `(z, x, y)` order.
    pub fn tile_pyramid(&self) -> Vec<WorldTileCoords> {
        self.tile_pyramid_for_source(None, None, None)
    }

    /// Deterministically enumerates all `WorldTileCoords` intersecting both `self.bounds` and an
    /// optional `source_bounds`, clamped to `source_min_zoom..=source_max_zoom`.
    pub fn tile_pyramid_for_source(
        &self,
        source_min_zoom: Option<u8>,
        source_max_zoom: Option<u8>,
        source_bounds: Option<(f64, f64, f64, f64)>,
    ) -> Vec<WorldTileCoords> {
        let z_start = self
            .min_zoom
            .max(source_min_zoom.unwrap_or(0))
            .min(MAX_ENUMERABLE_ZOOM);
        let z_end = self
            .max_zoom
            .min(source_max_zoom.unwrap_or(MAX_ENUMERABLE_ZOOM))
            .min(MAX_ENUMERABLE_ZOOM);
        if z_start > z_end {
            return Vec::new();
        }

        let [west, south, east, north] = self.bounds;
        let clamped_south = south.min(north).clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
        let clamped_north = north.max(south).clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);

        let src_lat_range = source_bounds.map(|(_, s, _, n)| {
            (
                s.min(n).clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT),
                n.max(s).clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT),
            )
        });
        let (eff_south, eff_north) = match src_lat_range {
            Some((s, n)) => {
                let low = clamped_south.max(s);
                let high = clamped_north.min(n);
                if low > high {
                    return Vec::new();
                }
                (low, high)
            }
            None => (clamped_south, clamped_north),
        };

        let lon_spans = intersect_lon_spans(lon_intervals(west, east), source_bounds);
        if lon_spans.is_empty() {
            return Vec::new();
        }

        let mut coords_set: BTreeSet<(u8, i32, i32)> = BTreeSet::new();
        for z in z_start..=z_end {
            let grid = 1_u32 << u32::from(z);
            let (y_min, y_max) = lat_tile_range(eff_south, eff_north, grid);
            for (lon_w, lon_e) in &lon_spans {
                let (x_min, x_max) = lon_tile_range(*lon_w, *lon_e, grid);
                for x in x_min..=x_max {
                    for y in y_min..=y_max {
                        coords_set.insert((z, x, y));
                    }
                }
            }
        }

        coords_set
            .into_iter()
            .map(|(z, x, y)| WorldTileCoords::from((x, y, ZoomLevel::new(z))))
            .collect()
    }
}

fn lon_intervals(west: f64, east: f64) -> Vec<(f64, f64)> {
    if (east - west) >= 360.0 || (west <= -180.0 && east >= 180.0) {
        return vec![(-180.0, 180.0)];
    }
    let w = west.clamp(-180.0, 180.0);
    let e = east.clamp(-180.0, 180.0);
    if w <= e {
        vec![(w, e)]
    } else {
        vec![(w, 180.0), (-180.0, e)]
    }
}

fn intersect_lon_spans(
    region_spans: Vec<(f64, f64)>,
    source_bounds: Option<(f64, f64, f64, f64)>,
) -> Vec<(f64, f64)> {
    let Some((sw, _, se, _)) = source_bounds else {
        return region_spans;
    };
    let src_spans = lon_intervals(sw, se);
    let mut out = Vec::new();
    for (rw, re) in &region_spans {
        for (xw, xe) in &src_spans {
            let low = rw.max(*xw);
            let high = re.min(*xe);
            if low <= high {
                out.push((low, high));
            }
        }
    }
    out
}

fn lon_tile_range(west: f64, east: f64, grid: u32) -> (i32, i32) {
    let n = f64::from(grid);
    let max_idx = (grid - 1) as i32;
    let frac_w = ((west + 180.0) / 360.0) * n;
    let x_min = (frac_w.floor() as i32).clamp(0, max_idx);
    if east <= west {
        return (x_min, x_min);
    }
    let frac_e = ((east + 180.0) / 360.0) * n;
    let adjusted_e = if frac_e > frac_w {
        frac_e - 1e-9
    } else {
        frac_e
    };
    let x_max = (adjusted_e.floor() as i32).clamp(x_min, max_idx);
    (x_min, x_max)
}

fn lat_to_mercator_frac(lat_deg: f64) -> f64 {
    let lat_rad = lat_deg
        .clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT)
        .to_radians();
    (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / PI) * 0.5
}

fn lat_tile_range(south: f64, north: f64, grid: u32) -> (i32, i32) {
    let n = f64::from(grid);
    let max_idx = (grid - 1) as i32;
    let frac_n = lat_to_mercator_frac(north) * n;
    let y_min = (frac_n.floor() as i32).clamp(0, max_idx);
    if north <= south {
        return (y_min, y_min);
    }
    let frac_s = lat_to_mercator_frac(south) * n;
    let adjusted_s = if frac_s > frac_n {
        frac_s - 1e-9
    } else {
        frac_s
    };
    let y_max = (adjusted_s.floor() as i32).clamp(y_min, max_idx);
    (y_min, y_max)
}

//! Property tests for Mercator projection, tile hierarchy, quadkeys, and zoom scales.

use proptest::prelude::*;

use crate::{
    LatLon, TileAddressingScheme, TileCoords, WorldCoords, WorldTileCoords, Zoom, ZoomLevel,
};

proptest! {
    #[test]
    fn prop_latlon_mercator_world_roundtrip_no_nan(
        lat in -85.05_f64..=85.05_f64,
        lon in -180.0_f64..=180.0_f64,
        zoom_val in 0.0_f64..=22.0_f64,
    ) {
        let lat_lon = LatLon::new(lat, lon);
        let merc = lat_lon.to_mercator();
        prop_assert!(merc.x.is_finite() && merc.y.is_finite());
        let back = LatLon::from_mercator(merc);
        prop_assert!(back.latitude.is_finite() && back.longitude.is_finite());
        prop_assert!((back.latitude - lat).abs() < 1e-5, "lat drift: {} vs {}", back.latitude, lat);
        prop_assert!((back.longitude - lon).abs() < 1e-5, "lon drift: {} vs {}", back.longitude, lon);

        let world = WorldCoords::from_lat_lon(lat_lon, Zoom::new(zoom_val));
        prop_assert!(world.x.is_finite() && world.y.is_finite());
    }
}

proptest! {
    #[test]
    fn prop_world_tile_hierarchy_containment(
        z_raw in 0_u8..=20_u8,
        x_frac in 0.0_f64..1.0_f64,
        y_frac in 0.0_f64..1.0_f64,
    ) {
        let bound = 1_i32 << z_raw;
        let x = (x_frac * f64::from(bound)).floor() as i32;
        let y = (y_frac * f64::from(bound)).floor() as i32;
        let parent = WorldTileCoords::from((x, y, ZoomLevel::new(z_raw)));
        let children = parent.get_children();
        for child in children {
            prop_assert_eq!(child.get_parent(), Some(parent));
            prop_assert_eq!(u8::from(child.z), z_raw + 1);
            prop_assert!(child.build_quad_key().is_some());
        }
        let parent_qk = parent.build_quad_key().expect("valid parent quadkey");
        let child_qk = children[3].build_quad_key().expect("valid child quadkey");
        prop_assert!(child_qk > parent_qk);
    }
}

proptest! {
    #[test]
    fn prop_quadkey_roundtrip(
        z_raw in 0_u8..=22_u8,
        x_frac in 0.0_f64..1.0_f64,
        y_frac in 0.0_f64..1.0_f64,
    ) {
        let bound = 1_u32 << z_raw;
        let x = ((x_frac * f64::from(bound)).floor() as u32).min(bound - 1);
        let y = ((y_frac * f64::from(bound)).floor() as u32).min(bound - 1);
        for scheme in [TileAddressingScheme::XYZ, TileAddressingScheme::TMS] {
            let tile = TileCoords::from((x, y, ZoomLevel::new(z_raw)));
            let world = tile.into_world_tile(scheme).expect("valid tile");
            let back = world.into_tile(scheme).expect("roundtrip tile");
            prop_assert_eq!(back, tile);
            prop_assert!(world.build_quad_key().is_some());
        }
    }
}

proptest! {
    #[test]
    fn prop_zoom_scale_monotonic_finite(
        z_lo in -2.0_f64..=24.0_f64,
        delta in 0.001_f64..=8.0_f64,
        target_level in 0_u8..=22_u8,
    ) {
        let lo = Zoom::new(z_lo);
        let hi = Zoom::new(z_lo + delta);
        let level = ZoomLevel::new(target_level);
        let s_lo = lo.scale_to_zoom_level(level);
        let s_hi = hi.scale_to_zoom_level(level);
        prop_assert!(s_lo.is_finite() && s_lo > 0.0);
        prop_assert!(s_hi.is_finite() && s_hi > 0.0);
        prop_assert!(s_hi < s_lo);
        prop_assert!(lo.scale_delta(&hi) > 1.0);
    }
}

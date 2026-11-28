#![allow(clippy::expect_used, clippy::panic)]

use super::{radius_pixels, style_zoom, MERCATOR_LATITUDE_LIMIT};
use crate::{coords::TILE_SIZE, projection::globe::globe_radius_pixels};

#[test]
fn inside_the_mercator_world_the_style_zoom_is_the_world_s() {
    for latitude in [
        -85.05,
        -60.0,
        0.0,
        27.765,
        70.0,
        85.0,
        MERCATOR_LATITUDE_LIMIT,
    ] {
        for zoom in [0.0, 2.5, 11.67, 17.0, 22.0] {
            let world = TILE_SIZE * 2_f64.powf(zoom);
            let today = globe_radius_pixels(world, latitude);
            assert!(
                (radius_pixels(zoom, latitude) / today - 1.0).abs() < 1e-12,
                "{latitude}, {zoom}: the radius departs from the world's"
            );
            // The inverse: a camera at the world's radius reads the world's zoom, so line widths,
            // text sizes and zoom ranges of a style stay as they are.
            assert!(
                (style_zoom(today, latitude) - zoom).abs() < 1e-12,
                "{latitude}, {zoom}: the zoom departs from the world's"
            );
            assert_eq!(style_zoom(today, latitude).floor(), zoom.floor());
        }
    }
}

#[test]
fn across_a_pole_the_style_zoom_changes_continuously_and_symmetrically() {
    // A fixed physical scale on a meridian from 80 degrees over the pole and back down to 80.
    let radius = radius_pixels(6.0, 80.0);
    let mut previous: Option<f64> = None;
    for step in 0..=2000 {
        let along = f64::from(step) * 0.01;
        let latitude = if along <= 10.0 {
            80.0 + along
        } else {
            100.0 - along
        };
        let zoom = style_zoom(radius, latitude);
        assert!(zoom.is_finite(), "{latitude}: {zoom}");
        if let Some(previous) = previous {
            // The parallel's cosine changes no faster than its latitude.
            assert!(
                (zoom - previous).abs() < 0.01_f64.to_radians() / 0.08 / std::f64::consts::LN_2,
                "{latitude}: the zoom jumps from {previous} to {zoom}"
            );
        }
        previous = Some(zoom);
        assert_eq!(
            zoom,
            style_zoom(radius, -latitude),
            "{latitude}: north and south differ"
        );
    }
    // Over the cap the zoom holds the value it reaches at the world's edge.
    let edge = style_zoom(radius, MERCATOR_LATITUDE_LIMIT);
    for latitude in [86.0, 88.0, 90.0] {
        assert_eq!(style_zoom(radius, latitude), edge);
    }
}

//! A line drawn over terrain keeps its style width on screen from the near edge to the
//! horizon, and terrain in front of it hides it.
use super::read_blocking;
use crate::{
    coords::{LatLon, WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer, map::HeadlessMap},
    render::RenderPlugin,
    style::Style,
};

/// The z12 tile north of the equator east of the prime meridian; it spans about 9.8 km.
fn root() -> WorldTileCoords {
    WorldTileCoords::from((2048, 2047, ZoomLevel::new(12)))
}

/// The point the camera looks at, 2.2 km inside the southern edge of [`root`].
const CENTER: LatLon = LatLon {
    latitude: 0.02,
    longitude: 0.044,
};
const PITCH_DEGREES: f64 = 80.0;
/// Zoom at which a 64-pixel frame at [`PITCH_DEGREES`] puts the eye 100 m above the center.
const ZOOM: f64 = 13.67;

/// Terrarium pixels of 0 m and of 150 m.
const SEA_LEVEL: image::Rgba<u8> = image::Rgba([128, 0, 0, 255]);
const RIDGE: image::Rgba<u8> = image::Rgba([128, 150, 0, 255]);

fn style(placement: &str) -> Style {
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "terrain": {"source": "dem"},
        "center": [CENTER.longitude, CENTER.latitude], "zoom": ZOOM, "pitch": PITCH_DEGREES,
        "sources": {
            "lines": {"type": "vector", "tiles": ["offline://lines"]},
            "dem": {"type": "raster-dem", "tiles": ["offline://dem"], "encoding": "terrarium"}
        },
        "layers": [
            {"id": "background", "type": "background",
             "paint": {"background-color": "#990000"}},
            {"id": "route", "type": "line", "source": "lines", "source-layer": "lines",
             "metadata": {"maplibre-rs:terrain-structure": placement},
             "paint": {"line-color": "#00ff00", "line-width": 3}}
        ]
    }))
    .expect("style");
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    style
}

/// A map looking north over `dem` at `lines`, placed as `placement` says.
async fn overlay_map(
    placement: &str,
    dem: image::RgbaImage,
    lines: serde_json::Value,
) -> HeadlessMap {
    let style = style(placement);
    let layers = style.layers[1..].to_vec();
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(
                crate::headless::HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.set_max_pitch(cgmath::Deg(85.0));
    let view = map.view_state();
    let altitude = view.camera_to_center_distance() * PITCH_DEGREES.to_radians().cos()
        / view.pixels_per_meter();
    assert!(
        (90.0..110.0).contains(&altitude),
        "eye is {altitude:.0} m above the ground"
    );
    map.load_dem_tiles(vec![(root(), dem)]).expect("DEM");
    // The source serves the lines at every tile the view asks for, as a GeoJSON source does.
    let mut processed = crate::headless::map::ProcessedLayers::default();
    for coords in map.required_tile_coords().expect("view tiles") {
        let mut tile = map
            .process_geojson(
                &lines,
                "lines",
                layers.clone(),
                coords,
                crate::projection::ProjectionType::Mercator,
            )
            .expect("lines");
        processed.append(&mut tile);
    }
    assert!(!processed.vector.is_empty(), "processed geometry");
    map.render_frames_with_terrain(processed, Vec::new(), Vec::new(), 16)
        .expect("frames");
    map
}

/// A meridian from `south` to `north` degrees of latitude, `east` degrees east of the center.
fn meridian(east: f64, south: f64, north: f64) -> [[f64; 2]; 2] {
    let longitude = CENTER.longitude + east;
    [[longitude, south], [longitude, north]]
}

fn lines(lines: &[[[f64; 2]; 2]]) -> serde_json::Value {
    serde_json::json!({"type": "Feature", "properties": {}, "geometry": {
        "type": "MultiLineString", "coordinates": lines
    }})
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> &[u8] {
    &pixels[(y * 64 + x) * 4..][..4]
}

fn is_green(pixel: &[u8]) -> bool {
    pixel[1] > 150 && pixel[0] < 100
}

/// How many pixels of `row` the green line covers, counting partly covered pixels by their
/// share of green over the red ground.
fn line_width(pixels: &[u8], row: usize) -> f64 {
    (0..64)
        .map(|x| pixel(pixels, x, row))
        .filter(|pixel| pixel[1] > pixel[0])
        .map(|pixel| f64::from(pixel[1]) / 255.0)
        .sum()
}

#[tokio::test]
async fn overlay_line_keeps_its_style_width_across_a_low_steep_view() {
    let flat = image::RgbaImage::from_pixel(2, 2, SEA_LEVEL);
    for (placement, expect_steady) in [("draped", false), ("overlay", true)] {
        let routes = lines(&[meridian(0.0, 0.01, 0.087)]);
        let map = overlay_map(placement, flat.clone(), routes).await;
        let pixels = read_blocking(&map);
        // Both measured rows must show the ground on either side of the line.
        for row in [32, 63] {
            let ground = pixel(&pixels, 4, row);
            assert!(
                ground[0] > 100 && ground[1] < 60,
                "{placement}: row {row} is {ground:?}, not ground"
            );
        }
        let bottom = line_width(&pixels, 63);
        let middle = line_width(&pixels, 32);
        let steady = [bottom, middle]
            .iter()
            .all(|width| (1.5..=6.0).contains(width));
        assert_eq!(
            steady, expect_steady,
            "{placement}: 3 px line is {bottom:.2} px wide at the bottom, {middle:.2} px mid-frame"
        );
    }
}

#[tokio::test]
async fn overlay_line_climbs_a_facing_slope_and_hides_behind_its_crest() {
    // Rows 22 and 23 of 32, from the north, raise a 150 m ridge: its face climbs from 0.6 km
    // north of the eye to a crest at 0.95 km, and its top stays level for another 300 m.
    let ridge = image::RgbaImage::from_fn(32, 32, |_, y| {
        if (22..24).contains(&y) {
            RIDGE
        } else {
            SEA_LEVEL
        }
    });
    // A line west of the center climbs the face onto the top. One east of the center crosses
    // the plain beyond the ridge, which the ridge hides from the eye.
    let routes = lines(&[
        meridian(-0.002, 0.016, 0.025),
        [
            [CENTER.longitude + 0.001, 0.032],
            [CENTER.longitude + 0.004, 0.032],
        ],
    ]);
    let map = overlay_map("overlay", ridge, routes.clone()).await;
    let pixels = read_blocking(&map);
    let width = |row: usize, columns: std::ops::Range<usize>| -> f64 {
        columns
            .map(|x| pixel(&pixels, x, row))
            .filter(|pixel| pixel[1] > pixel[0])
            .map(|pixel| f64::from(pixel[1]) / 255.0)
            .sum()
    };
    let near_rows: Vec<usize> = (0..64)
        .filter(|y| (0..32).any(|x| is_green(pixel(&pixels, x, *y))))
        .collect();
    let (Some(top), Some(bottom)) = (near_rows.first().copied(), near_rows.last().copied()) else {
        panic!("near line missing");
    };
    // Up the face as on the plain, every row between the ends holds the line's full width.
    let thin: Vec<(usize, f64)> = (top + 1..bottom)
        .map(|row| (row, width(row, 0..32)))
        .filter(|(_, width)| *width < 1.5)
        .collect();
    assert!(
        thin.is_empty(),
        "near line narrows or breaks on rows {thin:?}"
    );
    let far: Vec<usize> = (0..64)
        .filter(|y| (32..64).any(|x| is_green(pixel(&pixels, x, *y))))
        .collect();
    assert!(
        far.is_empty(),
        "far line shows through the ridge on rows {far:?}"
    );

    // On a plain the far line is in view, so the check above can fail.
    let flat = image::RgbaImage::from_pixel(32, 32, SEA_LEVEL);
    let map = overlay_map("overlay", flat, routes).await;
    let pixels = read_blocking(&map);
    let far = (0..64)
        .flat_map(|y| (32..64).map(move |x| (x, y)))
        .filter(|(x, y)| is_green(pixel(&pixels, *x, *y)))
        .count();
    assert!(far > 0, "far line is not drawn over flat terrain");
}

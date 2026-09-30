//! Heatmap density, ramp and opacity on known point distributions.
use geozero::mvt::Message;

use super::*;
use crate::{
    headless::map::process_tile_layers,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

/// Centre density of one unit-weight point: 1 / sqrt(2 * pi), so intensity 2.5066 makes it 1.
const UNIT_INTENSITY: f64 = 2.506_628_274_631_000_2;

/// A point layer with the given points as (x, y, weight) in tile units of a 4096 grid.
fn points_tile(points: &[(i32, i32, f64)]) -> Vec<u8> {
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            name: "points".into(),
            version: 2,
            extent: Some(4096),
            keys: vec!["mag".into()],
            values: points
                .iter()
                .map(|(_, _, weight)| geozero::mvt::tile::Value {
                    double_value: Some(*weight),
                    ..Default::default()
                })
                .collect(),
            features: points
                .iter()
                .enumerate()
                .map(|(index, (x, y, _))| geozero::mvt::tile::Feature {
                    tags: vec![0, index as u32],
                    r#type: Some(1),
                    geometry: vec![9, (*x as u32) << 1, (*y as u32) << 1],
                    ..Default::default()
                })
                .collect(),
        }],
    }
    .encode_to_vec()
}

fn heatmap_style(paint: serde_json::Value) -> Style {
    let coords = target();
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let lon = (f64::from(coords.x) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[lon,lat],"zoom":12.0,
        "sources":{"points":{"type":"vector","tiles":["offline://points"],"maxzoom":14}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
            {"id":"heat","type":"heatmap","source":"points","source-layer":"points","paint":paint}]
    }))
    .expect("style")
}

async fn rendered(style: Style, points: &[(i32, i32, f64)]) -> Vec<u8> {
    let layer = style
        .layers
        .iter()
        .find(|layer| layer.id == "heat")
        .expect("heatmap layer");
    let processed = process_tile_layers(
        &points_tile(points),
        layer,
        target(),
        crate::projection::ProjectionType::default(),
    )
    .expect("points");
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(HeatmapPlugin),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.render_frames_with_terrain(processed, vec![], vec![], 3)
        .expect("heatmap frame");
    read_blocking(&map, "heatmap")
}

fn pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * SIZE + x) * 4) as usize;
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]
}

const CENTRE: (i32, i32) = (2048, 2048);

fn ramp_at(density: f64) -> [u8; 4] {
    let ramp = crate::style::heatmap::HeatmapPaint::default().ramp();
    ramp[(density.clamp(0.0, 1.0) * 255.0).round() as usize]
}

/// A colour composited over white background.
fn over_white(premultiplied: [u8; 4], opacity: f64) -> [u8; 3] {
    let alpha = f64::from(premultiplied[3]) / 255.0 * opacity;
    [0, 1, 2].map(|channel| {
        (f64::from(premultiplied[channel]) * opacity + 255.0 * (1.0 - alpha)).round() as u8
    })
}

#[tokio::test]
async fn one_point_paints_the_ramp_colour_of_its_density_and_nothing_past_the_radius() {
    let paint = serde_json::json!({"heatmap-radius": 40, "heatmap-intensity": UNIT_INTENSITY});
    let bytes = rendered(heatmap_style(paint), &[(CENTRE.0, CENTRE.1, 1.0)]).await;
    let centre = pixel(&bytes, SIZE / 2, SIZE / 2);
    let expected = over_white(ramp_at(1.0), 1.0);
    assert!(
        centre[..3]
            .iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= 3),
        "centre density 1 is the last ramp colour: {centre:?} against {expected:?}"
    );
    let outside = pixel(&bytes, SIZE / 2 + 60, SIZE / 2);
    assert_eq!(
        outside,
        [255, 255, 255, 255],
        "beyond the radius is background"
    );
}

fn assert_close(actual: [u8; 4], expected: [u8; 3], tolerance: u8, what: &str) {
    assert!(
        actual[..3]
            .iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= tolerance),
        "{what}: {actual:?} against {expected:?}"
    );
}

fn kernel(distance_ratio: f64) -> f64 {
    // The GL JS kernel: a Gaussian three standard deviations wide across the radius.
    (-0.5 * 9.0 * distance_ratio * distance_ratio).exp()
}

#[tokio::test]
async fn density_falls_off_as_the_gaussian_kernel_between_centre_and_radius() {
    let paint = serde_json::json!({"heatmap-radius": 40, "heatmap-intensity": UNIT_INTENSITY});
    let bytes = rendered(heatmap_style(paint), &[(CENTRE.0, CENTRE.1, 1.0)]).await;
    for pixels in [10_u32, 20, 30] {
        // A pixel is sampled at its centre, half a pixel past its index.
        let density = kernel((f64::from(pixels) + 0.5) / 40.0);
        assert_close(
            pixel(&bytes, SIZE / 2 + pixels, SIZE / 2),
            over_white(ramp_at(density), 1.0),
            8,
            &format!("{pixels}px from the centre"),
        );
    }
}

#[tokio::test]
async fn overlapping_points_add_their_densities() {
    let paint = serde_json::json!({"heatmap-radius": 40, "heatmap-intensity": UNIT_INTENSITY});
    // Two points 40 px apart; between them each contributes exp(-4.5 * 0.25).
    let points = [
        (CENTRE.0 - 160, CENTRE.1, 1.0),
        (CENTRE.0 + 160, CENTRE.1, 1.0),
    ];
    let two = rendered(heatmap_style(paint.clone()), &points).await;
    let one = rendered(heatmap_style(paint), &points[..1]).await;
    let midpoint = pixel(&two, SIZE / 2, SIZE / 2);
    assert_close(
        midpoint,
        over_white(ramp_at(2.0 * kernel(0.5)), 1.0),
        8,
        "between two points",
    );
    assert_ne!(
        midpoint[..3],
        pixel(&one, SIZE / 2, SIZE / 2)[..3],
        "the second point makes the midpoint denser"
    );
}

#[tokio::test]
async fn a_data_driven_weight_scales_each_points_density() {
    let paint = serde_json::json!({
        "heatmap-radius": 20,
        "heatmap-intensity": UNIT_INTENSITY,
        "heatmap-weight": ["get", "mag"]
    });
    let points = [
        (CENTRE.0 - 400, CENTRE.1, 0.25),
        (CENTRE.0 + 400, CENTRE.1, 1.0),
    ];
    let bytes = rendered(heatmap_style(paint), &points).await;
    assert_close(
        pixel(&bytes, SIZE / 2 - 50, SIZE / 2),
        over_white(ramp_at(0.25), 1.0),
        6,
        "weight 0.25",
    );
    assert_close(
        pixel(&bytes, SIZE / 2 + 50, SIZE / 2),
        over_white(ramp_at(1.0), 1.0),
        6,
        "weight 1",
    );
}

#[tokio::test]
async fn heatmap_opacity_fades_the_layer_into_the_background() {
    let paint = serde_json::json!({
        "heatmap-radius": 40, "heatmap-intensity": UNIT_INTENSITY, "heatmap-opacity": 0.5
    });
    let bytes = rendered(heatmap_style(paint), &[(CENTRE.0, CENTRE.1, 1.0)]).await;
    assert_close(
        pixel(&bytes, SIZE / 2, SIZE / 2),
        over_white(ramp_at(1.0), 0.5),
        4,
        "half-opaque centre",
    );
}

#[tokio::test]
async fn radius_and_intensity_are_evaluated_at_the_view_zoom() {
    // At zoom 12 the stops give radius 40 and intensity of exactly one unit density.
    let paint = serde_json::json!({
        "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 0, 24, 80],
        "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0, 24, 2.0 * UNIT_INTENSITY]
    });
    let bytes = rendered(heatmap_style(paint), &[(CENTRE.0, CENTRE.1, 1.0)]).await;
    assert_close(
        pixel(&bytes, SIZE / 2, SIZE / 2),
        over_white(ramp_at(1.0), 1.0),
        3,
        "intensity at zoom 12",
    );
    assert_close(
        pixel(&bytes, SIZE / 2 + 20, SIZE / 2),
        over_white(ramp_at(kernel(20.5 / 40.0)), 1.0),
        8,
        "radius 40 at zoom 12",
    );
    assert_eq!(pixel(&bytes, SIZE / 2 + 60, SIZE / 2), [255, 255, 255, 255]);
}

/// A tile with the points and a polygon covering the whole tile, for layer-order fixtures.
fn points_and_land(points: &[(i32, i32, f64)]) -> Vec<u8> {
    let mut tile = geozero::mvt::Tile::decode(points_tile(points).as_slice()).expect("tile");
    tile.layers.push(geozero::mvt::tile::Layer {
        name: "land".into(),
        version: 2,
        extent: Some(4096),
        features: vec![geozero::mvt::tile::Feature {
            r#type: Some(3),
            geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
            ..Default::default()
        }],
        ..Default::default()
    });
    tile.encode_to_vec()
}

async fn layered(order: [&str; 2]) -> Vec<u8> {
    let heat = serde_json::json!({"id":"heat","type":"heatmap","source":"points",
        "source-layer":"points",
        "paint":{"heatmap-radius":40,"heatmap-intensity":UNIT_INTENSITY}});
    let land = serde_json::json!({"id":"land","type":"fill","source":"points",
        "source-layer":"land","paint":{"fill-color":"#0000ff"}});
    let layers: Vec<_> = order
        .iter()
        .map(|id| {
            if *id == "heat" {
                heat.clone()
            } else {
                land.clone()
            }
        })
        .collect();
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version":8,"zoom":12.0,
        "sources":{"points":{"type":"vector","tiles":["offline://points"],"maxzoom":14}},
        "layers":layers
    }))
    .expect("style");
    let coords = target();
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    style.center = Some([
        (f64::from(coords.x) + 0.5) / n * 360.0 - 180.0,
        (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
            .sinh()
            .atan()
            .to_degrees(),
    ]);
    let bytes = points_and_land(&[(CENTRE.0, CENTRE.1, 1.0)]);
    let mut processed = process_tile_layers(
        &bytes,
        &style.layers[0],
        coords,
        crate::projection::ProjectionType::default(),
    )
    .expect("first layer");
    processed.append(
        &mut process_tile_layers(
            &bytes,
            &style.layers[1],
            coords,
            crate::projection::ProjectionType::default(),
        )
        .expect("second layer"),
    );
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(HeatmapPlugin),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.render_frames_with_terrain(processed, vec![], vec![], 3)
        .expect("frame");
    read_blocking(&map, "heatmap-order")
}

#[tokio::test]
async fn a_heatmap_composites_at_its_place_in_the_style_order() {
    let over_land = layered(["land", "heat"]).await;
    assert_close(
        pixel(&over_land, SIZE / 2, SIZE / 2),
        {
            // The ramp's last colour is opaque red, so it replaces the land beneath.
            [255, 0, 0]
        },
        3,
        "a heatmap above the land",
    );
    assert_close(
        pixel(&over_land, SIZE / 2 + 80, SIZE / 2),
        [0, 0, 255],
        2,
        "the land shows beyond the kernel",
    );
    let under_land = layered(["heat", "land"]).await;
    assert_close(
        pixel(&under_land, SIZE / 2, SIZE / 2),
        [0, 0, 255],
        2,
        "a heatmap below opaque land is covered",
    );
}

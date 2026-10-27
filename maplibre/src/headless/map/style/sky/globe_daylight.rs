//! A globe drawn without terrain turns from day to night under the host's sun, as terrain does,
//! and an eclipse takes its day side toward its night; the sky around it is left alone.
#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    celestial::{EcefDirection, VisibleFraction},
    headless::map::{readback::read_blocking, HeadlessMap},
    render::RenderPlugin,
    style::Style,
};

const SIZE: u32 = 256;

async fn globe() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":0.6,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#c0c0c0"}}],"projection":{"type":"vertical-perspective"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
        ],
    )
    .expect("map");
    for _ in 0..3 {
        map.run_frame().expect("frame");
    }
    map
}

fn frame(map: &mut HeadlessMap) -> Vec<u8> {
    map.run_frame().expect("frame");
    read_blocking(
        map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn luminance(pixels: &[u8], x: u32, y: u32) -> f64 {
    let [r, g, b, _] = pixel(pixels, x, y);
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}

/// The columns of the globe's left and right limbs along the middle row.
fn limbs(pixels: &[u8]) -> (u32, u32) {
    let row = SIZE / 2;
    let drawn: Vec<u32> = (0..SIZE)
        .filter(|&x| pixel(pixels, x, row)[3] > 0)
        .collect();
    (
        *drawn.first().expect("the globe is drawn"),
        *drawn.last().expect("the globe is drawn"),
    )
}

/// The sun over 0°N 60°E lights the globe's centre, 0°N 0°E, from 30° up and leaves its west
/// in night; the night keeps a share of the day's light, and the sky beside the globe is not
/// drawn on.
#[tokio::test]
async fn the_globe_without_terrain_turns_from_day_to_night() {
    let mut map = globe().await;
    let unlit = frame(&mut map);
    let longitude = 60_f64.to_radians();
    map.set_sun_direction(Some(
        EcefDirection::new(longitude.cos(), longitude.sin(), 0.0).expect("sun"),
    ));
    let lit = frame(&mut map);
    let (left, right) = limbs(&unlit);
    let row = SIZE / 2;
    let (west, centre) = (left + (right - left) / 8, SIZE / 2);
    let report = format!(
        "limbs {left}..{right}: centre {:.0} → {:.0}, west {:.0} → {:.0}",
        luminance(&unlit, centre, row),
        luminance(&lit, centre, row),
        luminance(&unlit, west, row),
        luminance(&lit, west, row),
    );
    assert!(
        (luminance(&lit, centre, row) - luminance(&unlit, centre, row)).abs() < 3.0,
        "the day side keeps its colour: {report}"
    );
    let night = luminance(&lit, west, row) / luminance(&unlit, west, row);
    assert!(
        (0.25..0.5).contains(&night),
        "the night side keeps about a third of its light: {report}"
    );
    assert_eq!(
        pixel(&lit, 2, 2),
        pixel(&unlit, 2, 2),
        "the sky beside the globe is left alone"
    );

    // Totality where the eye stands takes the day side to the night's light.
    map.set_sun_visible_fraction(VisibleFraction::new(0.0).expect("fraction"));
    let total = frame(&mut map);
    let darkened = luminance(&total, centre, row) / luminance(&unlit, centre, row);
    assert!(
        (0.25..0.5).contains(&darkened),
        "the day side dims to its night in totality, {darkened:.2}: {report}"
    );
}

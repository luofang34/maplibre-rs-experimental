//! A host draws the globe's atmosphere without a style `sky`, lit from the sun it sets.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::InnerSpace;

use crate::{
    celestial::EcefDirection,
    headless::map::{readback::read_blocking, HeadlessMap},
    render::{view_state::CameraOptions, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;

async fn globe() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":0,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#204060"}}],"projection":{"type":"globe"}}"##).expect("style");
    let (kernel, renderer) =
        crate::headless::create_headless_renderer(SIZE, SIZE, Default::default())
            .await
            .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
        ],
    )
    .expect("map")
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

/// The summed colour of the pixels left and right of the centre that the globe leaves
/// uncovered, where only the atmosphere can draw.
fn halo(pixels: &[u8], space: &[bool]) -> (u64, u64) {
    let mut sides = (0, 0);
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if !space[index] {
            continue;
        }
        let colour: u64 = pixel[..3].iter().map(|&c| u64::from(c)).sum();
        if (index as u32 % SIZE) < SIZE / 2 {
            sides.0 += colour;
        } else {
            sides.1 += colour;
        }
    }
    sides
}

#[tokio::test]
async fn the_host_atmosphere_shines_on_the_side_of_its_sun() {
    let mut map = globe().await;
    let plain = frame(&mut map);
    let space: Vec<bool> = plain
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[3] == 0)
        .collect();
    let globe_pixels = space.iter().filter(|&&empty| !empty).count();
    assert!(
        globe_pixels > 1000 && globe_pixels < space.len() / 2,
        "the globe must sit inside the frame: {globe_pixels} pixels"
    );
    assert_eq!(halo(&plain, &space), (0, 0), "no sky, no atmosphere");

    map.set_atmosphere(true);
    // The sun over 0°N 90°E lights the eastern limb, on the right of a view north up.
    map.set_sun_direction(Some(EcefDirection::new(0.0, 1.0, 0.0).expect("direction")));
    let (west, east) = halo(&frame(&mut map), &space);
    assert!(
        east > 2 * west.max(1),
        "east {east} must outshine west {west}"
    );

    map.set_sun_direction(Some(EcefDirection::new(0.0, -1.0, 0.0).expect("direction")));
    let (west, east) = halo(&frame(&mut map), &space);
    assert!(
        west > 2 * east.max(1),
        "west {west} must outshine east {east}"
    );

    map.set_atmosphere(false);
    assert_eq!(halo(&frame(&mut map), &space), (0, 0));
}

/// The eye's height above the sea-level sphere, in metres.
fn eye_altitude(map: &HeadlessMap) -> f64 {
    let camera =
        crate::render::projection::globe_camera_for_view(map.view_state()).expect("camera");
    (camera.camera_position().magnitude() - 1.0) * crate::projection::globe::EARTH_RADIUS_METERS
}

/// The summed colour of the frame's upper third, where an eye looking at the horizon sees sky.
fn sky_brightness(pixels: &[u8]) -> u64 {
    pixels[..(SIZE * SIZE / 3 * 4) as usize]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[..3].iter().map(|&c| u64::from(c)).sum::<u64>())
        .sum()
}

/// Sunrise at 41.07204° N, 71.85733° W on 2026-06-14 at 09:19:41Z, the sun's centre on the
/// geometric horizon at azimuth 58.40°: seen from about 100 m up over a vertical-perspective
/// globe, the sky is brightest towards it.
#[tokio::test]
async fn the_dawn_sky_is_brightest_towards_the_rising_sun() {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-71.85733,41.07204],"zoom":3,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#101010"}}],"projection":{"type":"vertical-perspective"}}"##).expect("style");
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
    map.set_max_pitch(cgmath::Deg(85.0));
    map.set_atmosphere(true);
    map.set_sun_direction(Some(
        EcefDirection::new(0.70214, 0.59237, 0.39509).expect("sun"),
    ));
    let pose = |map: &mut HeadlessMap, zoom: f64, bearing: f64| {
        map.jump_to(CameraOptions {
            zoom: Some(zoom),
            bearing: Some(cgmath::Deg(bearing)),
            pitch: Some(cgmath::Deg(85.0)),
            ..Default::default()
        })
        .expect("jump");
    };
    // The eye's height falls as the zoom rises; halve the bracket to about 100 m.
    let (mut low, mut high) = (10.0, 24.0);
    for _ in 0..40 {
        let zoom = (low + high) / 2.0;
        pose(&mut map, zoom, 58.4);
        if eye_altitude(&map) > 100.0 {
            low = zoom;
        } else {
            high = zoom;
        }
    }
    let altitude = eye_altitude(&map);
    assert!((altitude - 100.0).abs() < 5.0, "eye at {altitude} m");

    let mut skies = Vec::new();
    for bearing in [58.4, 148.4, 238.4, 328.4] {
        pose(&mut map, low, bearing);
        skies.push((bearing, sky_brightness(&frame(&mut map))));
    }
    let others = skies[1..].iter().map(|(_, sky)| *sky).max().expect("skies");
    assert!(
        skies[0].1 * 5 > others * 6,
        "towards the sun the sky must be the brightest by a fifth: {skies:?}"
    );
    assert!(
        skies[2].1 > 0,
        "the sky away from the sun still scatters some light"
    );
}

/// The mean brightness and saturation, the spread of the colour channels, of a band of rows.
fn rows_colour(pixels: &[u8], rows: std::ops::Range<u32>) -> (f64, f64) {
    let band = &pixels[(rows.start * SIZE * 4) as usize..(rows.end * SIZE * 4) as usize];
    let (mut brightness, mut saturation) = (0.0, 0.0);
    for pixel in band.as_chunks::<4>().0 {
        let channels = pixel[..3].iter().map(|&c| f64::from(c));
        brightness += channels.clone().sum::<f64>() / 3.0;
        saturation += channels.clone().fold(0.0, f64::max) - channels.fold(255.0, f64::min);
    }
    let count = f64::from((rows.end - rows.start) * SIZE);
    (brightness / count, saturation / count)
}

/// The atmosphere lies behind the terrain: at dawn the ground under the eye stays grey and
/// dim while the sky over the horizon glows.
#[tokio::test]
async fn the_dawn_atmosphere_does_not_tint_the_ground() {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[-71.85733,41.07204],"zoom":15,"pitch":80,"bearing":58.4,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#808080"}}],"terrain":{"source":"dem"},"projection":{"type":"vertical-perspective"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
        SIZE,
        SIZE,
        Default::default(),
        crate::io::resource_loader::SharedLoader::new(super::daylight::FlatDem),
    )
    .await
    .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
        ],
    )
    .expect("map");
    map.set_max_pitch(cgmath::Deg(85.0));
    map.set_atmosphere(true);
    map.set_sun_direction(Some(
        EcefDirection::new(0.70214, 0.59237, 0.39509).expect("sun"),
    ));
    let mut pixels = Vec::new();
    for _ in 0..40 {
        pixels = frame(&mut map);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    let (ground, ground_saturation) = rows_colour(&pixels, SIZE * 9 / 10..SIZE);
    let horizon_row = (0..SIZE)
        .rev()
        .find(|row| {
            let at = ((row * SIZE + SIZE / 2) * 4) as usize;
            pixels[at + 3] < 255
        })
        .expect("the frame shows sky");
    let (sky, sky_saturation) = rows_colour(
        &pixels,
        horizon_row.saturating_sub(SIZE / 16)..horizon_row + 1,
    );
    assert!(
        ground < sky && ground_saturation * 4.0 < sky_saturation,
        "ground ({ground}, saturation {ground_saturation}) must be dimmer and greyer than \
         the horizon ({sky}, saturation {sky_saturation})"
    );
    assert!(
        ground_saturation < 8.0,
        "the ground is tinted: {ground_saturation}"
    );
}

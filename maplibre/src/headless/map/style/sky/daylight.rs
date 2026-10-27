//! The host's sun lights the terrain: the ground under the sun is drawn as by day, its
//! antipode at the night brightness, readable and cooler rather than black, raised by a lit
//! Moon above it, and the terminator in between.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3};

use crate::{
    celestial::{Disc, EcefDirection, TwilightBand, NIGHT_BRIGHTNESS},
    coords::LatLon,
    headless::map::{readback::read_blocking, HeadlessMap},
    render::{view_state::CameraOptions, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 128;

/// Serves every elevation tile as flat ground at sea level.
#[derive(Clone)]
pub(super) struct FlatDem;

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl crate::io::source_client::HttpClient for FlatDem {
    async fn fetch(
        &self,
        _url: &str,
    ) -> Result<Vec<u8>, crate::io::source_client::SourceFetchError> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(256, 256, image::Rgba([128, 0, 0, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("PNG");
        Ok(png.into_inner())
    }
}

async fn globe() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":3,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"encoding":"terrarium"}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#c8c8c8"}}],"terrain":{"source":"dem"},"projection":{"type":"globe"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
        SIZE,
        SIZE,
        Default::default(),
        crate::io::resource_loader::SharedLoader::new(FlatDem),
    )
    .await
    .expect("renderer");
    HeadlessMap::new(
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
    .expect("map")
}

/// The summed colour channels of the pixel at the centre of the view once the terrain has
/// loaded and two frames agree.
async fn center_brightness(map: &mut HeadlessMap) -> u32 {
    center_color(map).await.iter().map(|&c| u32::from(c)).sum()
}

/// The colour of the pixel at the centre of the view once the terrain has loaded and two
/// frames agree.
async fn center_color(map: &mut HeadlessMap) -> [u8; 3] {
    let mut previous = None;
    for _ in 0..40 {
        for _ in 0..4 {
            map.run_frame().expect("frame");
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }
        let pixels = read_blocking(
            map,
            map.head_texture().expect("head texture"),
            wgpu::TextureAspect::All,
        )
        .expect("readback");
        let at = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        let color = [pixels[at], pixels[at + 1], pixels[at + 2]];
        if color != [0; 3] && previous == Some(color) {
            return color;
        }
        previous = Some(color);
    }
    panic!("the frame never settled");
}

fn look_at(map: &mut HeadlessMap, longitude: f64) {
    look_at_place(map, LatLon::new(0.0, longitude));
}

fn look_at_place(map: &mut HeadlessMap, place: LatLon) {
    map.jump_to(CameraOptions {
        center: Some(place),
        ..Default::default()
    })
    .expect("jump");
}

fn sun(x: f64, y: f64, z: f64) -> Option<EcefDirection> {
    Some(EcefDirection::new(x, y, z).expect("direction"))
}

#[tokio::test]
async fn the_subsolar_point_is_lit_and_its_antipode_dark() {
    let mut map = globe().await;
    let unlit = center_brightness(&mut map).await;

    // The sun over 0°N 0°E, the center of the view.
    map.set_sun_direction(sun(1.0, 0.0, 0.0));
    let subsolar = center_brightness(&mut map).await;
    assert_eq!(subsolar, unlit, "the sun overhead leaves the day's colour");

    // The same sun seen from the antipode, 0°N 180°E.
    look_at(&mut map, 180.0);
    let antipode = center_brightness(&mut map).await;
    assert!(
        antipode * 3 < subsolar * 2,
        "the antipode {antipode} must be darker than the subsolar point {subsolar}"
    );

    // With the sun over the north pole the equator lies on the terminator, at an elevation
    // inside the twilight band.
    look_at(&mut map, 0.0);
    map.set_sun_direction(sun(0.0, 0.0, 1.0));
    let terminator = center_brightness(&mut map).await;
    assert!(
        antipode < terminator && terminator < subsolar,
        "the terminator {terminator} lies between night {antipode} and day {subsolar}"
    );

    // A band ending night above the horizon puts the same ground into full night.
    map.set_twilight_band(TwilightBand::new(1.0, 3.0, NIGHT_BRIGHTNESS).expect("band"));
    let night = center_brightness(&mut map).await;
    assert!(night.abs_diff(antipode) <= 3, "{night} != {antipode}");

    map.set_sun_direction(None);
    assert_eq!(center_brightness(&mut map).await, unlit);
}

/// The ECEF unit vector of a place, from the axes' definition.
fn ecef(place: LatLon) -> Vector3<f64> {
    let (latitude, longitude) = (place.latitude.to_radians(), place.longitude.to_radians());
    Vector3::new(
        latitude.cos() * longitude.cos(),
        latitude.cos() * longitude.sin(),
        latitude.sin(),
    )
}

fn place(ecef: Vector3<f64>) -> LatLon {
    LatLon::new(
        ecef.z.asin().to_degrees(),
        ecef.y.atan2(ecef.x).to_degrees(),
    )
}

/// Sunrise at 41.07204° N, 71.85733° W on 2026-06-14 at 09:19:41Z, when the sun's centre
/// crosses that observer's geometric horizon at azimuth 58.40°, over 23.27° N 40.15° E.
#[tokio::test]
async fn sunrise_over_long_island_lies_on_the_terminator() {
    let sun = Vector3::new(0.70214, 0.59237, 0.39509);
    let observer = ecef(LatLon::new(41.07204, -71.85733));
    let elevation = observer.dot(sun.normalize()).asin().to_degrees();
    assert!(
        elevation.abs() < 0.01,
        "the sun stands {elevation}° over the observer"
    );
    let mut map = globe().await;
    look_at_place(&mut map, place(observer));
    let unlit = center_brightness(&mut map).await;
    map.set_sun_direction(Some(
        EcefDirection::new(sun.x, sun.y, sun.z).expect("direction"),
    ));
    look_at_place(&mut map, place(-sun.normalize()));
    let night = center_brightness(&mut map).await;

    look_at_place(&mut map, place(observer));
    let dawn = center_brightness(&mut map).await;
    assert!(
        night < dawn && dawn < unlit,
        "the observer {dawn} lies between night {night} and day {unlit}"
    );

    // Ten degrees of arc along the great circle through the observer and the subsolar point.
    let towards_sun = (sun.normalize() - observer * observer.dot(sun.normalize())).normalize();
    let (sine, cosine) = 10_f64.to_radians().sin_cos();
    look_at_place(&mut map, place(observer * cosine + towards_sun * sine));
    assert_eq!(
        center_brightness(&mut map).await,
        unlit,
        "sunward is full day"
    );
    look_at_place(&mut map, place(observer * cosine - towards_sun * sine));
    let away = center_brightness(&mut map).await;
    assert!(
        away.abs_diff(night) <= 3,
        "away from the sun {away} is night {night}"
    );
}

/// Night is darker and cooler than day but never black, so a night map stays legible.
#[tokio::test]
async fn the_night_side_stays_readable_and_cooler() {
    let mut map = globe().await;
    let [r, g, b] = center_color(&mut map).await;
    let day = f64::from(r) + f64::from(g) + f64::from(b);
    map.set_sun_direction(sun(1.0, 0.0, 0.0));
    look_at(&mut map, 180.0);
    let [nr, ng, nb] = center_color(&mut map).await;
    let night = f64::from(nr) + f64::from(ng) + f64::from(nb);
    assert!(
        (0.25..=0.5).contains(&(night / day)),
        "night keeps {:.2} of the day's light",
        night / day
    );
    assert!(nb > nr, "night turns toward blue: {nr} {ng} {nb}");
}

fn moon(x: f64, y: f64, z: f64) -> Option<Disc> {
    Some(Disc::new(EcefDirection::new(x, y, z).expect("direction"), 0.26).expect("disc"))
}

/// A Moon above the night side lights it by its lit share: a full Moon overhead brightens it,
/// a new Moon or one below the horizon adds nothing.
#[tokio::test]
async fn a_full_moon_overhead_lights_the_night_side() {
    let mut map = globe().await;
    // The sun over 0°N 0°E; the view's centre, 0°N 180°E, is the middle of the night.
    map.set_sun_direction(sun(1.0, 0.0, 0.0));
    look_at(&mut map, 180.0);
    let night = center_brightness(&mut map).await;

    // Opposite the sun the Moon is full, and it stands overhead at 0°N 180°E.
    map.set_moon_disc(moon(-1.0, 0.0, 0.0));
    let moonlit = center_brightness(&mut map).await;
    assert!(
        moonlit > night + night / 3,
        "a full Moon overhead lifts night {night} to {moonlit}"
    );

    // The same full Moon cannot light ground from below its horizon.
    map.set_moon_disc(moon(0.6, 0.0, -0.8));
    let below = center_brightness(&mut map).await;
    assert!(below.abs_diff(night) <= 3, "{below} != {night}");

    // Beside the sun the Moon is new and lights nothing.
    map.set_sun_direction(sun(-0.6, 0.0, 0.8));
    map.set_moon_disc(moon(-0.6, 0.0, 0.8));
    let new_moon_night = {
        map.set_moon_disc(None);
        center_brightness(&mut map).await
    };
    map.set_moon_disc(moon(-0.6, 0.0, 0.8));
    let new_moon = center_brightness(&mut map).await;
    assert!(
        new_moon.abs_diff(new_moon_night) <= 3,
        "a new Moon {new_moon} adds nothing to {new_moon_night}"
    );
}

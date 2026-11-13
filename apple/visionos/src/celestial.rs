//! Explicit planetary body, sun/moon lighting, atmosphere, and starfield C ABI.

use maplibre::{
    celestial::{AngularRadius, Disc, EcefDirection, Star, TwilightBand},
    projection::body::Body,
};

use crate::MaplibreVisionOSMap;

/// Packed stride in `f32` elements per star: `[dir_x, dir_y, dir_z, v_mag, r, g, b]`.
const STAR_STRIDE_F32: usize = 7;

/// Sets the display pixel ratio (`physical_pixels / logical_points`, e.g. `2.0` on Retina)
/// so starfield discs, SDF text halos, and symbol layout scale accurately.
///
/// # Safety
///
/// `map` must be a valid `MaplibreVisionOSMap` handle or null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_pixel_ratio(
    map: *mut MaplibreVisionOSMap,
    ratio: f64,
) {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return;
    };
    if ratio.is_finite() && ratio > 0.25 {
        handle.map.set_pixel_ratio(ratio);
    }
}

/// Explicitly declares the planetary body rendered by the map (`radius_meters`,
/// `highest_ground_meters`, `lowest_ground_meters`). Callers must never rely on an implicit
/// Earth default when presenting a specific body.
///
/// # Safety
///
/// `map` must be a valid `MaplibreVisionOSMap` handle or null.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_body(
    map: *mut MaplibreVisionOSMap,
    radius_meters: f64,
    highest_ground_meters: f64,
    lowest_ground_meters: f64,
) {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return;
    };
    if !radius_meters.is_finite()
        || radius_meters <= 0.0
        || !highest_ground_meters.is_finite()
        || !lowest_ground_meters.is_finite()
    {
        return;
    }
    handle.map.set_body(Body {
        radius_meters,
        highest_ground_meters,
        lowest_ground_meters,
    });
}

/// Updates the body-fixed Sun direction + angular radius, Moon disc, twilight band, and
/// atmospheric limb scattering for the current frame.
///
/// # Safety
///
/// `map` must be a valid `MaplibreVisionOSMap` handle or null. When `has_sun` is true,
/// `sun_ecef_xyz` must point to 3 `f64` values; when `has_moon` is true, `moon_ecef_xyz`
/// must point to 3 `f64` values.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_sky(
    map: *mut MaplibreVisionOSMap,
    has_sun: bool,
    sun_ecef_xyz: *const f64,
    sun_radius_deg: f64,
    has_moon: bool,
    moon_ecef_xyz: *const f64,
    moon_radius_deg: f64,
    night_brightness: f32,
    atmosphere: bool,
) {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return;
    };
    let sun_dir = if has_sun && !sun_ecef_xyz.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(sun_ecef_xyz, 3) };
        EcefDirection::new(slice[0], slice[1], slice[2]).ok()
    } else {
        None
    };
    handle.map.set_sun_direction(sun_dir);
    handle.map.set_sun_angular_radius(
        sun_dir
            .is_some()
            .then_some(sun_radius_deg)
            .and_then(|deg| AngularRadius::new(deg).ok()),
    );
    let moon_disc = if has_moon && !moon_ecef_xyz.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(moon_ecef_xyz, 3) };
        EcefDirection::new(slice[0], slice[1], slice[2])
            .ok()
            .and_then(|dir| Disc::new(dir, moon_radius_deg).ok())
    } else {
        None
    };
    handle.map.set_moon_disc(moon_disc);
    if let Ok(band) = TwilightBand::new(-6.0, 2.0, night_brightness) {
        handle.map.set_twilight_band(band);
    }
    handle.map.set_atmosphere(atmosphere);
}

/// Replaces the celestial starfield drawn behind the globe. Each star in `packed_stars_f32`
/// occupies 7 consecutive `f32` values: `[dir_x, dir_y, dir_z, v_mag, r, g, b]` in body-fixed
/// (`ECEF` / principal-axis) unit direction coordinates. Setting `r < 0.0` selects default white.
///
/// # Safety
///
/// `map` must be a valid `MaplibreVisionOSMap` handle or null. When `star_count > 0`,
/// `packed_stars_f32` must address at least `star_count * 7` readable `f32` values.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_set_stars(
    map: *mut MaplibreVisionOSMap,
    packed_stars_f32: *const f32,
    star_count: u32,
) {
    let Some(handle) = (unsafe { map.as_mut() }) else {
        return;
    };
    if star_count == 0 || packed_stars_f32.is_null() {
        handle.map.set_stars(Vec::new());
        return;
    }
    let total = (star_count as usize).saturating_mul(STAR_STRIDE_F32);
    let raw = unsafe { std::slice::from_raw_parts(packed_stars_f32, total) };
    let mut stars = Vec::with_capacity(star_count as usize);
    for chunk in raw.as_chunks::<STAR_STRIDE_F32>().0 {
        let Some(dir) = EcefDirection::new(
            f64::from(chunk[0]),
            f64::from(chunk[1]),
            f64::from(chunk[2]),
        )
        .ok() else {
            continue;
        };
        let color = if chunk[4] >= 0.0 {
            Some([chunk[4], chunk[5], chunk[6]])
        } else {
            None
        };
        if let Ok(star) = Star::new(dir, chunk[3], color) {
            stars.push(star);
        }
    }
    handle.map.set_stars(stars);
}

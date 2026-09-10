#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use indicate_instrument_state::{AircraftState, FreshnessPolicy, Sig, resolve};

fn data() -> PanelData {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    data.track_rad = Sig::valid(90_f32.to_radians());
    data.gs_kt = Sig::valid(100.0);
    data.vsi_fpm = Sig::valid(-300.0);
    data
}

#[test]
fn unavailable_data_cannot_emit_directional_guidance() {
    let missing = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    assert_eq!(directions(&missing).length, 0);
    let mut invalid = data();
    invalid.track_rad = Sig::valid(f32::NAN);
    let scene = directions(&invalid);
    assert!(
        scene.strokes[..scene.length as usize]
            .iter()
            .flat_map(|s| s.a.into_iter().chain(s.b))
            .all(f32::is_finite)
    );
}

#[test]
fn velocity_wings_use_gravity_even_when_aircraft_is_banked() {
    let mut scene = AngularScene {
        length: 0,
        strokes: [AngularStroke::default(); 1024],
    };
    let center = normalized([50.0, 20.0, -4.0]).expect("velocity");
    marker(&mut scene, center, false);
    assert_eq!(scene.length, 35);
    let wing = scene.strokes[32];
    assert!((wing.b[2] - wing.a[2]).abs() < 1e-7);
    marker(&mut scene, scale(center, -1.0), true);
    assert_eq!(scene.length, 72);
    let mean = |offset: usize| {
        normalized((offset..offset + 32).fold([0.0; 3], |sum, i| add(sum, scene.strokes[i].a)))
            .expect("ring")
    };
    assert!(vector::dot(mean(0), mean(35)) < -0.9999);
}

#[test]
fn compass_has_true_north_when_looking_away_from_track() {
    let scene = directions(&data());
    // North stays at the north world direction even while ownship travels east.
    let north_tick = scene.strokes[0];
    assert!(vector::dot(north_tick.a, [0.0, 1.0, 0.0]) > 0.9999);
    let east_view = [1.0, 0.0, 0.0];
    assert!(vector::dot(north_tick.a, east_view).abs() < 1e-6);
    // A projected direction reconstructed through either rolled eye retains that world direction.
    for roll in [-1.0_f32, 0.0, 0.7] {
        let screen_x = libm::cosf(roll) * north_tick.b[0] + libm::sinf(roll) * north_tick.b[2];
        let screen_y = -libm::sinf(roll) * north_tick.b[0] + libm::cosf(roll) * north_tick.b[2];
        assert!(
            (screen_x * libm::cosf(roll) - screen_y * libm::sinf(roll) - north_tick.b[0]).abs()
                < 1e-6
        );
    }
}

#[test]
fn attitude_requires_true_heading_and_scene_remains_bounded() {
    let mut data = data();
    let baseline = directions(&data).length;
    data.roll_rad = Sig::valid(0.5);
    data.pitch_rad = Sig::valid(0.1);
    assert_eq!(directions(&data).length, baseline);
    data.heading.value_rad = Sig::valid(1.4);
    data.heading.reference = HeadingReference::Magnetic;
    assert_eq!(directions(&data).length, baseline);
    data.heading.reference = HeadingReference::True;
    let scene = directions(&data);
    assert!(scene.length > baseline && scene.length < 1024);
    assert_eq!(core::mem::size_of::<AngularScene>(), 24580);
}

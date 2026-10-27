use super::star_instance;
use crate::celestial::{EcefDirection, Star};

fn star(magnitude: f32) -> Star {
    Star::new(
        EcefDirection::new(1.0, 0.0, 0.0).expect("direction"),
        magnitude,
        None,
    )
    .expect("star")
}

#[test]
fn brighter_stars_are_drawn_larger_and_no_dimmer() {
    let instances: Vec<_> = [6.0, 3.0, 1.0, -1.5]
        .into_iter()
        .map(|magnitude| star_instance(&star(magnitude)))
        .collect();
    for pair in instances.windows(2) {
        assert!(pair[1].direction_radius[3] >= pair[0].direction_radius[3]);
        assert!(pair[1].color_brightness[3] >= pair[0].color_brightness[3]);
    }
    assert!(
        instances[0].color_brightness[3] < 0.2,
        "a faint star stays faint"
    );
    assert_eq!(instances[2].color_brightness[3], 1.0);
    assert!(
        instances[3].direction_radius[3] <= 4.0,
        "the brightest stars are capped"
    );
}

#[test]
fn stars_are_uploaded_in_globe_world_axes() {
    // 0°N 0°E lies on the globe's +z axis.
    let instance = star_instance(&star(1.0));
    assert_eq!(&instance.direction_radius[..3], &[0.0, 0.0, 1.0]);
    assert_eq!(&instance.color_brightness[..3], &[1.0, 1.0, 1.0]);
}

#[test]
fn stars_with_an_impossible_magnitude_or_colour_are_refused() {
    let direction = EcefDirection::new(0.0, 0.0, 1.0).expect("direction");
    assert!(Star::new(direction, f32::NAN, None).is_err());
    assert!(Star::new(direction, 1.0, Some([1.5, 0.0, 0.0])).is_err());
    assert!(Star::new(direction, 1.0, Some([1.0, 0.5, 0.0])).is_ok());
}

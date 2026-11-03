#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn readback_normalization_preserves_alpha_and_recovers_straight_colors() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("frame.png");
    let pixels = [0, 0, 0, 0, 50, 25, 0, 128, 20, 40, 60, 255];
    RgbaImage::from_raw(3, 1, pixels.to_vec())
        .expect("image dimensions")
        .save(&path)
        .expect("write readback");
    unpremultiply(&path).expect("normalize readback");
    assert_eq!(
        image::open(&path)
            .expect("normalized image")
            .to_rgba8()
            .as_raw(),
        &[0, 0, 0, 0, 100, 50, 0, 128, 20, 40, 60, 255]
    );
    composite_opaque_background(&path, [255, 255, 255]).expect("composite on white");
    assert_eq!(
        image::open(&path)
            .expect("composited image")
            .to_rgba8()
            .as_raw(),
        &[255, 255, 255, 255, 177, 152, 127, 255, 20, 40, 60, 255]
    );
}

#[test]
fn image_comparison_measures_every_channel_and_marks_changed_pixels() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let diff = directory.path().join("diff.png");
    let expected = RgbaImage::from_pixel(2, 1, Rgba([20, 40, 60, 255]));
    let mut actual = expected.clone();
    actual.put_pixel(1, 0, Rgba([30, 20, 60, 250]));
    let difference = compare_equal_dimensions(&actual, &expected, &diff, 1.0).expect("compare");
    assert!((difference.mean - 35.0 / (8.0 * 255.0)).abs() < f64::EPSILON);
    assert_eq!(difference.solid, 0);
    let image = image::open(diff).expect("diff image").to_rgba8();
    assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
    assert_eq!(image.get_pixel(1, 0).0, [255, 0, 0, 20]);
}

/// A blue frame with white drawn wherever `white` holds, as a fill with something missing.
fn blue_with_white(white: impl Fn(u32, u32) -> bool) -> RgbaImage {
    RgbaImage::from_fn(512, 512, |x, y| {
        if white(x, y) {
            Rgba([255, 255, 255, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    })
}

#[test]
fn a_hole_in_a_fill_fails_though_the_mean_difference_is_small() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let diff = directory.path().join("diff.png");
    let expected = blue_with_white(|_, _| false);
    // A 46 px hole where a globe fill misses its pole cap.
    let hole = blue_with_white(|x, y| {
        let (dx, dy) = (f64::from(x) - 256.0, f64::from(y) - 196.0);
        dx.hypot(dy) <= 23.0
    });

    let difference = compare_equal_dimensions(&hole, &expected, &diff, 1.0).expect("compare");

    let limits = Difference::limits(0.02, 1.0);
    assert!(difference.mean < limits.mean, "{difference}");
    assert!(difference.solid > 500, "{difference}");
    assert!(!difference.within(limits), "{difference}");
}

#[test]
fn shifted_edges_and_thin_strokes_leave_no_solid_region() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let diff = directory.path().join("diff.png");
    let expected = blue_with_white(|x, _| x < 200);
    // The fill's edge lands 8 px over and a 6 px wide label stroke is missing.
    let actual = blue_with_white(|x, y| x < 208 || ((300..306).contains(&x) && y < 400));

    let difference = compare_equal_dimensions(&actual, &expected, &diff, 1.0).expect("compare");

    assert_eq!(difference.solid, 0, "{difference}");
    assert!(
        difference.within(Difference::limits(0.02, 1.0)),
        "{difference}"
    );
}

#[test]
fn the_solid_region_square_grows_with_the_pixel_ratio() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let diff = directory.path().join("diff.png");
    let expected = blue_with_white(|x, _| x < 200);
    let actual = blue_with_white(|x, _| x < 212);

    let at_one = compare_equal_dimensions(&actual, &expected, &diff, 1.0).expect("compare");
    let at_two = compare_equal_dimensions(&actual, &expected, &diff, 2.0).expect("compare");

    assert!(at_one.solid > 0, "{at_one}");
    assert_eq!(at_two.solid, 0, "{at_two}");
}

#[tokio::test(flavor = "multi_thread")]
async fn translucent_fill_preserves_color_and_alpha_in_gpu_readback() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fill-color/opacity");
    let directory = tempfile::tempdir().expect("temporary render fixture");
    let mut style: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("style.json")).expect("fixture style"))
            .expect("valid style");
    let paint = &mut style["layers"][0]["paint"];
    paint["fill-color"] = serde_json::json!("rgba(128,0,0,0.5)");
    paint
        .as_object_mut()
        .expect("paint object")
        .remove("fill-opacity");
    std::fs::write(
        directory.path().join("style.json"),
        serde_json::to_vec(&style).expect("serialize style"),
    )
    .expect("write style");
    let mut expected = image::open(fixture.join("expected.png"))
        .expect("fixture image")
        .to_rgba8();
    for pixel in expected.pixels_mut().filter(|pixel| pixel.0[3] > 0) {
        pixel.0[0] = 128;
    }
    expected
        .save(directory.path().join("expected.png"))
        .expect("write expected");

    let outcome = crate::run_test_inner(directory.path()).await;
    assert!(matches!(outcome, crate::TestResult::Pass(_)), "{outcome:?}");
    let actual = image::open(directory.path().join("actual.png"))
        .expect("rendered image")
        .to_rgba8();
    let pixel = actual.get_pixel(32, 32).0;
    for (actual, expected) in pixel.into_iter().zip([128, 0, 0, 128]) {
        assert!(actual.abs_diff(expected) <= 1, "center pixel: {pixel:?}");
    }
}

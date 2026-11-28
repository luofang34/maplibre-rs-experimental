use super::*;

#[test]
fn screen_raster_alpha_preserves_lower_vector_color() {
    crate::headless::map::test_async::run_async(async {
        for samples in [1, 4] {
            let map = alpha_map(false, samples, 1.0, Some(128)).await;
            assert_color(
                &pixels_blocking(&map, "raster-over-fill"),
                [128, 0, 127, 255],
            );
        }
    })
}

#[test]
fn terrain_raster_alpha_preserves_lower_vector_color() {
    crate::headless::map::test_async::run_async(async {
        for samples in [1, 4] {
            let map = alpha_map(true, samples, 1.0, Some(128)).await;
            assert_color(
                &pixels_blocking(&map, "raster-over-fill"),
                [128, 0, 127, 255],
            );
        }
    })
}

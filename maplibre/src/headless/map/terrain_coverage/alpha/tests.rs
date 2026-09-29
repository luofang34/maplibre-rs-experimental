use super::*;

#[tokio::test]
async fn screen_raster_alpha_preserves_lower_vector_color() {
    for samples in [1, 4] {
        let map = alpha_map(false, samples, 1.0, Some(128)).await;
        assert_color(
            &pixels_blocking(&map, "raster-over-fill"),
            [128, 0, 127, 255],
        );
    }
}

#[tokio::test]
async fn terrain_raster_alpha_preserves_lower_vector_color() {
    for samples in [1, 4] {
        let map = alpha_map(true, samples, 1.0, Some(128)).await;
        assert_color(
            &pixels_blocking(&map, "raster-over-fill"),
            [128, 0, 127, 255],
        );
    }
}

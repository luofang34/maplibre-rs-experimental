#![allow(clippy::expect_used, clippy::panic)]

use super::*;

#[tokio::test]
async fn png_failure_releases_the_buffer_for_the_next_capture() {
    let (_, renderer) = crate::headless::create_headless_renderer(3, 2, None)
        .await
        .expect("headless renderer");
    let Head::Headless(texture) = renderer.resources.surface.head() else {
        panic!("offscreen surface");
    };
    let directory = std::env::temp_dir();
    let result = write_surface_buffer_blocking(
        texture,
        &renderer.device,
        Some(directory.to_str().expect("test directory path")),
    );
    assert!(matches!(result, Err(SystemError::Image(_))));
    write_surface_buffer_blocking(texture, &renderer.device, None)
        .expect("the failed PNG write must not leave the capture buffer mapped");
}

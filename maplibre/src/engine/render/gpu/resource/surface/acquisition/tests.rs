use wgpu::CurrentSurfaceTexture;

use super::{acquire, SurfaceAcquireError};

#[test]
fn unavailable_surface_does_not_reconfigure() {
    for (status, expected) in [
        (CurrentSurfaceTexture::Timeout, SurfaceAcquireError::Timeout),
        (
            CurrentSurfaceTexture::Occluded,
            SurfaceAcquireError::Occluded,
        ),
        (CurrentSurfaceTexture::Lost, SurfaceAcquireError::Lost),
        (
            CurrentSurfaceTexture::Validation,
            SurfaceAcquireError::Validation,
        ),
    ] {
        let mut status = Some(status);
        let mut reconfigurations = 0_u32;
        let result = acquire(
            || status.take().unwrap_or(CurrentSurfaceTexture::Validation),
            || reconfigurations = reconfigurations.wrapping_add(1),
        );
        assert_eq!(result.err(), Some(expected));
        assert_eq!(reconfigurations, 0);
    }
}

#[test]
fn outdated_surface_retries_once_and_preserves_the_result() {
    let mut acquisitions = 0_u32;
    let mut reconfigurations = 0_u32;
    let result = acquire(
        || {
            acquisitions = acquisitions.wrapping_add(1);
            if acquisitions == 1 {
                CurrentSurfaceTexture::Outdated
            } else {
                CurrentSurfaceTexture::Lost
            }
        },
        || reconfigurations = reconfigurations.wrapping_add(1),
    );
    assert_eq!(result.err(), Some(SurfaceAcquireError::Lost));
    assert_eq!(acquisitions, 2);
    assert_eq!(reconfigurations, 1);
}

#[test]
fn repeated_outdated_surface_does_not_loop() {
    let mut acquisitions = 0_u32;
    let mut reconfigurations = 0_u32;
    let result = acquire(
        || {
            acquisitions = acquisitions.wrapping_add(1);
            CurrentSurfaceTexture::Outdated
        },
        || reconfigurations = reconfigurations.wrapping_add(1),
    );
    assert_eq!(result.err(), Some(SurfaceAcquireError::Outdated));
    assert_eq!(acquisitions, 2);
    assert_eq!(reconfigurations, 1);
}

//! Surface acquisition with one bounded reconfiguration attempt.

use wgpu::{CurrentSurfaceTexture, SurfaceTexture};

pub use crate::render::error::SurfaceAcquireError;

pub(super) fn acquire(
    mut next: impl FnMut() -> CurrentSurfaceTexture,
    mut configure: impl FnMut(),
) -> Result<SurfaceTexture, SurfaceAcquireError> {
    for attempt in 0..2 {
        match next() {
            CurrentSurfaceTexture::Success(texture) => return Ok(texture),
            CurrentSurfaceTexture::Timeout => return Err(SurfaceAcquireError::Timeout),
            CurrentSurfaceTexture::Occluded => return Err(SurfaceAcquireError::Occluded),
            CurrentSurfaceTexture::Lost => return Err(SurfaceAcquireError::Lost),
            CurrentSurfaceTexture::Validation => return Err(SurfaceAcquireError::Validation),
            CurrentSurfaceTexture::Suboptimal(texture) => {
                // Reconfiguration requires every outstanding surface texture to be dropped.
                drop(texture);
            }
            CurrentSurfaceTexture::Outdated => {}
        }
        if attempt == 0 {
            configure();
        }
    }
    Err(SurfaceAcquireError::Outdated)
}

#[cfg(test)]
mod tests;

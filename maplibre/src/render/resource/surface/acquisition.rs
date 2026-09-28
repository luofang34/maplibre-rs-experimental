//! Surface acquisition with one bounded reconfiguration attempt.

use thiserror::Error;
use wgpu::{CurrentSurfaceTexture, SurfaceTexture};

/// Why a window cannot supply a frame.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAcquireError {
    /// The compositor did not supply an image before its deadline.
    #[error("surface acquisition timed out")]
    Timeout,
    /// The compositor cannot display the window.
    #[error("surface is occluded")]
    Occluded,
    /// The surface still needs configuration after a retry.
    #[error("surface configuration remains outdated")]
    Outdated,
    /// The host must recreate the surface.
    #[error("surface was lost")]
    Lost,
    /// The device reported a validation error during acquisition.
    #[error("surface acquisition failed validation")]
    Validation,
}

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

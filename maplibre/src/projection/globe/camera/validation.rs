//! Checks on the inputs a globe camera is built from.

use super::{GlobeCameraError, GlobeCameraOptions};

pub(super) fn validate_options(options: GlobeCameraOptions) -> Result<(), GlobeCameraError> {
    if !options.width.is_finite()
        || !options.height.is_finite()
        || options.width <= 0.0
        || options.height <= 0.0
    {
        return Err(GlobeCameraError::InvalidViewport {
            width: options.width,
            height: options.height,
        });
    }
    if !options.field_of_view_degrees.is_finite()
        || !(0.0..180.0).contains(&options.field_of_view_degrees)
    {
        return Err(GlobeCameraError::InvalidFieldOfView {
            degrees: options.field_of_view_degrees,
        });
    }
    if !options.world_size.is_finite() || options.world_size <= 0.0 {
        return Err(GlobeCameraError::InvalidWorldSize {
            world_size: options.world_size,
        });
    }
    validate_angles(options)?;
    if !options.center_offset.x.is_finite() || !options.center_offset.y.is_finite() {
        return Err(GlobeCameraError::InvalidCenterOffset {
            x: options.center_offset.x,
            y: options.center_offset.y,
        });
    }
    Ok(())
}

fn validate_angles(options: GlobeCameraOptions) -> Result<(), GlobeCameraError> {
    for (name, degrees) in [
        ("center latitude", options.center.latitude),
        ("center longitude", options.center.longitude),
        ("bearing", options.bearing_degrees),
        ("pitch", options.pitch_degrees),
        ("roll", options.roll_degrees),
    ] {
        if !degrees.is_finite() {
            return Err(GlobeCameraError::InvalidAngle { name, degrees });
        }
    }
    Ok(())
}

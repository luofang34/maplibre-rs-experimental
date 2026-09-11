//! Aircraft or explicitly track-level reference for a collimated virtual instrument panel.
use super::{HeadingReference, PanelData, vector::*};

/// Local east/north/up axes; independent of the observer's head and either eye.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct ViewReference {
    /// 0: unavailable; 1: level track reference; 2: measured aircraft attitude and heading.
    pub kind: u32,
    /// Horizontal panel axis.
    pub right: [f32; 3],
    /// Vertical panel axis.
    pub up: [f32; 3],
    /// Panel forward axis.
    pub forward: [f32; 3],
}

/// Resolves a panel reference without substituting track for measured aircraft attitude.
pub fn view_reference(data: &PanelData) -> ViewReference {
    let aircraft = data.heading.value_rad.status.shows_value()
        && matches!(
            data.heading.reference,
            HeadingReference::True | HeadingReference::SimLocalTrue
        )
        && data.pitch_rad.status.shows_value()
        && data.roll_rad.status.shows_value();
    if !aircraft && !data.track_rad.status.shows_value() {
        return ViewReference::default();
    }
    let (heading, pitch, roll) = if aircraft {
        (
            data.heading.value_rad.value,
            data.pitch_rad.value,
            data.roll_rad.value,
        )
    } else {
        (data.track_rad.value, 0.0, 0.0)
    };
    if ![heading, pitch, roll].iter().all(|v| v.is_finite()) {
        return ViewReference::default();
    }
    let forward = direction(heading, pitch);
    let level = direction(heading + core::f32::consts::FRAC_PI_2, 0.0);
    let right = add(
        scale(level, libm::cosf(roll)),
        scale(cross(level, forward), -libm::sinf(roll)),
    );
    ViewReference {
        kind: if aircraft { 2 } else { 1 },
        right,
        up: cross(right, forward),
        forward,
    }
}

#[cfg(test)]
mod tests;

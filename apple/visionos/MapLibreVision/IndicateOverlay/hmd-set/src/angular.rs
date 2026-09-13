//! Collimated directions in local east/north/up coordinates, without a viewer-dependent basis.
use crate::panel::live;
use indicate_instrument_state::{HeadingReference, PanelData};

mod compass;
mod reference;
pub use reference::{ViewReference, view_reference};
mod vector;
use vector::{add, cross, direction, normalized, scale};

/// One angular segment. The host projects both endpoints as directions, with homogeneous w=0.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct AngularStroke {
    /// Start direction in local east/north/up coordinates.
    pub a: [f32; 3],
    /// End direction in local east/north/up coordinates.
    pub b: [f32; 3],
}

/// Bounded scene, independent of either eye and head pose.
#[repr(C)]
pub struct AngularScene {
    /// Number of initialized segments. Zero indicates unavailable world alignment.
    pub length: u32,
    /// Segments beyond `length` have no meaning.
    pub strokes: [AngularStroke; 1024],
}

impl AngularScene {
    fn line(&mut self, a: [f32; 3], b: [f32; 3]) {
        if !a.iter().chain(b.iter()).all(|v| v.is_finite()) {
            return;
        }
        if let Some(stroke) = self.strokes.get_mut(self.length as usize) {
            *stroke = AngularStroke { a, b };
            self.length = self.length.wrapping_add(1);
        }
    }
}

/// Produces true-bearing compass, velocity, retrograde, and available aircraft attitude cues.
/// Magnetic heading alone cannot locate a symbol in the true-north world frame.
pub fn directions(data: &PanelData) -> AngularScene {
    let mut scene = AngularScene {
        length: 0,
        strokes: [AngularStroke::default(); 1024],
    };
    if live(data.track_rad) {
        compass::draw(&mut scene);
        compass::bug(&mut scene, data.track_rad.value, false);
    }
    let true_heading = live(data.heading.value_rad)
        && matches!(
            data.heading.reference,
            HeadingReference::True | HeadingReference::SimLocalTrue
        );
    if true_heading {
        compass::bug(&mut scene, data.heading.value_rad.value, true);
    }
    if live(data.gs_kt) && live(data.vsi_fpm) && live(data.track_rad) && data.gs_kt.value > 2.0 {
        let horizontal = scale(
            direction(data.track_rad.value, 0.0),
            data.gs_kt.value * (1852.0 / 3600.0),
        );
        if let Some(velocity) = normalized(add(
            horizontal,
            [0.0, 0.0, data.vsi_fpm.value * (0.3048 / 60.0)],
        )) {
            marker(&mut scene, velocity, false);
            marker(&mut scene, scale(velocity, -1.0), true);
        }
    }
    if true_heading && live(data.roll_rad) && live(data.pitch_rad) {
        attitude(&mut scene, data);
    }
    scene
}

fn marker(scene: &mut AngularScene, center: [f32; 3], retrograde: bool) {
    // Gravity fixes the ring's wings. Head roll and aircraft bank cannot rotate this earth reference.
    let Some(right) = normalized(cross(center, [0.0, 0.0, 1.0])) else {
        return;
    };
    let up = cross(right, center);
    let point = |x, y| add(center, scale(add(scale(right, x), scale(up, y)), 0.0096));
    for i in 0..32 {
        let a = i as f32 * core::f32::consts::TAU / 32.0;
        let b = (i + 1) as f32 * core::f32::consts::TAU / 32.0;
        scene.line(
            point(libm::cosf(a), libm::sinf(a)),
            point(libm::cosf(b), libm::sinf(b)),
        );
    }
    for [x, y, u, v] in [
        [-2.0, 0.0, -1.0, 0.0],
        [1.0, 0.0, 2.0, 0.0],
        [0.0, 1.0, 0.0, 1.7],
    ] {
        scene.line(point(x, y), point(u, v));
    }
    if retrograde {
        scene.line(point(-0.65, -0.65), point(0.65, 0.65));
        scene.line(point(-0.65, 0.65), point(0.65, -0.65));
    }
}

fn attitude(scene: &mut AngularScene, data: &PanelData) {
    let frame = view_reference(data);
    // The waterline denotes the fuselage, distinct from the circular velocity marker.
    let point = |x, y| {
        add(
            frame.forward,
            add(scale(frame.right, x), scale(frame.up, y)),
        )
    };
    for [x, y, u, v] in [
        [-0.035, 0.0, -0.016, 0.0],
        [-0.016, 0.0, -0.008, -0.008],
        [-0.008, -0.008, 0.0, 0.0],
        [0.0, 0.0, 0.008, -0.008],
        [0.008, -0.008, 0.016, 0.0],
        [0.016, 0.0, 0.035, 0.0],
    ] {
        scene.line(point(x, y), point(u, v));
    }
    // The compact recovery attitude supplies pitch without cluttering its primary readouts.
    if !data.presentation.unusual {
        pitch_ladder(scene, data.heading.value_rad.value);
    }
}

fn pitch_ladder(scene: &mut AngularScene, heading: f32) {
    for pitch in (-30_i32..=30).step_by(5).filter(|p| *p != 0) {
        for side in [-1.0, 1.0] {
            for x in (2..8).filter(|x| pitch > 0 || x % 2 == 0) {
                scene.line(
                    direction(
                        heading + (side * x as f32).to_radians(),
                        (pitch as f32).to_radians(),
                    ),
                    direction(
                        heading + (side * (x + 1) as f32).to_radians(),
                        (pitch as f32).to_radians(),
                    ),
                );
            }
        }
        compass::label(
            scene,
            heading + 10_f32.to_radians(),
            (pitch as f32).to_radians(),
            pitch,
        );
    }
}

#[cfg(test)]
mod tests;

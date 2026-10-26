//! The path an animated camera move takes, as GL JS `easeTo` and `flyTo` draw it on the globe.
//!
//! A path gives the [`CameraOptions`] a move shows at each point of its progress; a jump to them
//! places the camera, in either navigation mode, so a move and a jump agree on where the camera
//! stands and how high its center rests.

use std::time::Duration;

use cgmath::Deg;

use super::{CameraOptions, ViewState};
use crate::{
    coords::{LatLon, TILE_SIZE},
    projection::globe::interaction::animation::{GlobeEase, GlobeFly},
    style::expression::Interpolation,
};

/// How a move travels to its destination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlightCurve {
    /// Center and zoom change together, as GL JS `easeTo`, over `duration`.
    Ease {
        /// How long the move takes.
        duration: Duration,
    },
    /// The camera zooms out to cross the distance and in again, as GL JS `flyTo`.
    Fly(FlyOptions),
}

/// How a [`FlightCurve::Fly`] move flies, with GL JS `flyTo`'s defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlyOptions {
    /// How far the camera zooms out along the way; GL JS's `curve`, 1.42 by default.
    pub curve: f64,
    /// Screenfuls per second the move averages; GL JS's `speed`, 1.2 by default.
    pub speed: f64,
    /// How long the move takes, instead of what `speed` gives.
    pub duration: Option<Duration>,
    /// The lowest zoom the camera zooms out to.
    pub min_zoom: f64,
}

impl Default for FlyOptions {
    fn default() -> Self {
        Self {
            curve: 1.42,
            speed: 1.2,
            duration: None,
            min_zoom: 0.0,
        }
    }
}

/// GL JS `defaultEasing`, the CSS `ease` curve.
const EASING: [f64; 4] = [0.25, 0.1, 0.25, 1.0];

/// One end of a move.
#[derive(Clone, Copy, Debug)]
struct Stop {
    center: LatLon,
    zoom: f64,
    bearing: f64,
    pitch: f64,
    roll: f64,
}

/// Where the center travels and how the zoom changes on the way.
#[derive(Clone, Copy, Debug)]
enum Course {
    Ease(GlobeEase),
    Fly { globe: GlobeFly, span: Span },
}

/// GL JS `flyTo`'s visible span `w(s)` and distance travelled `u(s)`, both relative to the start.
#[derive(Clone, Copy, Debug)]
enum Span {
    /// The camera rises and descends, van Wijk and Nuij's optimal path.
    Arc { r0: f64, rho: f64, w0: f64, u1: f64 },
    /// The center stays; the camera only zooms, `k` saying which way.
    Zoom { rho: f64, k: f64 },
}

impl Span {
    fn scale(&self, s: f64) -> f64 {
        match *self {
            Self::Arc { r0, rho, .. } => (r0 + rho * s).cosh() / r0.cosh(),
            Self::Zoom { rho, k } => (-k * rho * s).exp(),
        }
    }

    fn center_factor(&self, s: f64) -> f64 {
        match *self {
            Self::Arc { r0, rho, w0, u1 } => {
                w0 * (r0.cosh() * (r0 + rho * s).tanh() - r0.sinh()) / (rho * rho) / u1
            }
            Self::Zoom { .. } => 0.0,
        }
    }
}

/// An animated move from where the camera is to a destination.
#[derive(Clone, Copy, Debug)]
pub struct FlightPath {
    from: Stop,
    to: Stop,
    course: Course,
    /// The path's length in GL JS's `ρ`-screenfuls, for a fly.
    length: f64,
    duration: Duration,
}

impl FlightPath {
    /// How long the move takes.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// What the camera shows `elapsed` into the move, its center resting at `elevation`, the
    /// destination's height eased in from `start_elevation`; at its end, the destination exactly.
    pub fn at(&self, elapsed: Duration, start_elevation: f64, elevation: f64) -> CameraOptions {
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
        };
        if t >= 1.0 {
            return self.to.options(elevation);
        }
        let k = Interpolation::CubicBezier {
            control_points: EASING,
        }
        .factor(t, 0.0, 1.0);
        let target = match self.course {
            Course::Ease(ease) => ease.sample(k),
            Course::Fly { globe, span } => {
                let s = k * self.length;
                globe.sample(k, span.scale(s), span.center_factor(s))
            }
        };
        let mix = |from: f64, to: f64| from + (to - from) * k;
        Stop {
            center: target.center,
            zoom: target.zoom,
            bearing: mix(self.from.bearing, self.to.bearing),
            pitch: mix(self.from.pitch, self.to.pitch),
            roll: mix(self.from.roll, self.to.roll),
        }
        .options(mix(start_elevation, elevation))
    }

    /// The destination's center and zoom.
    pub(crate) fn destination(&self) -> (LatLon, f64) {
        (self.to.center, self.to.zoom)
    }

    /// Whether `elapsed` reaches the end of the move.
    pub fn arrived(&self, elapsed: Duration) -> bool {
        elapsed >= self.duration
    }
}

impl Stop {
    fn options(&self, elevation: f64) -> CameraOptions {
        CameraOptions {
            center: Some(self.center),
            zoom: Some(self.zoom),
            bearing: Some(Deg(self.bearing)),
            pitch: Some(Deg(self.pitch)),
            roll: Some(Deg(self.roll)),
            elevation: Some(elevation),
        }
    }
}

/// `to` turned by whole turns to lie within half a turn of `from`, as GL JS `_normalizeBearing`.
fn nearest_turn(to: f64, from: f64) -> f64 {
    from + (to - from + 180.0).rem_euclid(360.0) - 180.0
}

impl ViewState {
    /// The path a move to `options` takes along `curve`, from where the camera is now, the
    /// camera's own values standing in for those left out.
    pub fn flight_path(&self, options: CameraOptions, curve: FlightCurve) -> FlightPath {
        let start = self.resolve_jump(CameraOptions::default());
        let end = self.resolve_jump(options);
        let from = Stop {
            center: start.center,
            zoom: start.zoom.value(),
            bearing: start.bearing.0,
            pitch: start.pitch.0,
            roll: start.roll.0,
        };
        let to = Stop {
            center: end.center,
            zoom: end.zoom.value(),
            bearing: nearest_turn(end.bearing.0, from.bearing),
            pitch: end.pitch.0,
            roll: nearest_turn(end.roll.0, from.roll),
        };
        match curve {
            FlightCurve::Ease { duration } => FlightPath {
                from,
                to,
                course: Course::Ease(GlobeEase::new(
                    from.center,
                    from.zoom,
                    to.center,
                    Some(to.zoom),
                )),
                length: 0.0,
                duration,
            },
            FlightCurve::Fly(fly) => self.fly_path(from, to, fly),
        }
    }

    /// GL JS `flyTo`'s path from `from` to `to`.
    fn fly_path(&self, from: Stop, to: Stop, fly: FlyOptions) -> FlightPath {
        let world_size = TILE_SIZE * 2_f64.powf(from.zoom);
        let globe = GlobeFly::new(
            world_size,
            from.center,
            from.zoom,
            to.center,
            Some(to.zoom),
            fly.min_zoom,
        );
        let w0 = self.width().max(self.height());
        let w1 = w0 / globe.scale_of_zoom;
        let u1 = globe.pixel_path_length;
        let w_max = w0 / globe.scale_of_min_zoom;
        let rho = fly.curve.min((w_max / u1 * 2.0).sqrt());
        let rho2 = rho * rho;
        let zoom_out = |descent: bool| {
            let sign = if descent { -1.0 } else { 1.0 };
            let b = (w1 * w1 - w0 * w0 + sign * rho2 * rho2 * u1 * u1)
                / (2.0 * if descent { w1 } else { w0 } * rho2 * u1);
            ((b * b + 1.0).sqrt() - b).ln()
        };
        let r0 = zoom_out(false);
        let mut length = (zoom_out(true) - r0) / rho;
        let mut span = Span::Arc { r0, rho, w0, u1 };
        if u1.abs() < 0.000_002 || !length.is_finite() {
            if (w0 - w1).abs() < 0.000_001 {
                // Nowhere to fly: GL JS eases there instead.
                return self.flight_path(
                    CameraOptions {
                        elevation: None,
                        ..to.options(0.0)
                    },
                    FlightCurve::Ease {
                        duration: fly.duration.unwrap_or(Duration::ZERO),
                    },
                );
            }
            let k = if w1 < w0 { -1.0 } else { 1.0 };
            length = (w1 / w0).ln().abs() / rho;
            span = Span::Zoom { rho, k };
        }
        let duration = fly
            .duration
            .unwrap_or_else(|| Duration::from_secs_f64((length / fly.speed).max(0.0)));
        FlightPath {
            from,
            to,
            course: Course::Fly { globe, span },
            length,
            duration,
        }
    }
}

#[cfg(test)]
mod tests;

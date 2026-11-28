//! Interpolation curves for `interpolate` expressions.

/// How an `interpolate` expression moves between two stops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interpolation {
    /// Straight line between the stops.
    Linear,
    /// Exponential curve with the given base; a base of one is linear.
    Exponential {
        /// Base of the curve.
        base: f64,
    },
    /// Cubic bezier easing with two control points.
    CubicBezier {
        /// The two control points, `[x1, y1, x2, y2]`.
        control_points: [f64; 4],
    },
}

impl Interpolation {
    /// Where `input` lies between `lower` and `upper`, in `0..=1`.
    pub fn factor(&self, input: f64, lower: f64, upper: f64) -> f64 {
        match self {
            Self::Linear => exponential_progress(input, 1.0, lower, upper),
            Self::Exponential { base } => exponential_progress(input, *base, lower, upper),
            Self::CubicBezier { control_points } => {
                let [x1, y1, x2, y2] = *control_points;
                UnitBezier::new(x1, y1, x2, y2)
                    .solve(exponential_progress(input, 1.0, lower, upper))
            }
        }
    }
}

fn exponential_progress(input: f64, base: f64, lower: f64, upper: f64) -> f64 {
    let difference = upper - lower;
    let progress = input - lower;
    if difference == 0.0 {
        0.0
    } else if base == 1.0 {
        progress / difference
    } else {
        (base.powf(progress) - 1.0) / (base.powf(difference) - 1.0)
    }
}

/// A cubic bezier through `(0, 0)` and `(1, 1)`, solved for `y` at a given `x`.
struct UnitBezier {
    cx: f64,
    bx: f64,
    ax: f64,
    cy: f64,
    by: f64,
    ay: f64,
}

impl UnitBezier {
    fn new(p1x: f64, p1y: f64, p2x: f64, p2y: f64) -> Self {
        let cx = 3.0 * p1x;
        let bx = 3.0 * (p2x - p1x) - cx;
        let ax = 1.0 - cx - bx;
        let cy = 3.0 * p1y;
        let by = 3.0 * (p2y - p1y) - cy;
        let ay = 1.0 - cy - by;
        Self {
            cx,
            bx,
            ax,
            cy,
            by,
            ay,
        }
    }

    fn sample_x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn sample_y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn sample_x_derivative(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn solve_x(&self, x: f64) -> f64 {
        const EPSILON: f64 = 1e-6;
        let mut t = x;
        for _ in 0..8 {
            let error = self.sample_x(t) - x;
            if error.abs() < EPSILON {
                return t;
            }
            let derivative = self.sample_x_derivative(t);
            if derivative.abs() < 1e-6 {
                break;
            }
            t -= error / derivative;
        }
        let (mut low, mut high) = (0.0, 1.0);
        t = x;
        if t < low {
            return low;
        }
        if t > high {
            return high;
        }
        while low < high {
            let sampled = self.sample_x(t);
            if (sampled - x).abs() < EPSILON {
                return t;
            }
            if x > sampled {
                low = t;
            } else {
                high = t;
            }
            t = (high - low) * 0.5 + low;
        }
        t
    }

    fn solve(&self, x: f64) -> f64 {
        self.sample_y(self.solve_x(x))
    }
}

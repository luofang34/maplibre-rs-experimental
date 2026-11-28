//! Colour-space blending and linear numeric interpolation for expressions and composite values.

use std::f64::consts::PI;

use super::Color;

/// Linear blend of two numbers.
pub fn interpolate_number(from: f64, to: f64, t: f64) -> f64 {
    from * (1.0 - t) + to * t
}

/// Colour space an `interpolate` expression blends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// Straight RGB, the `interpolate` operator.
    Rgb,
    /// CIELAB, the `interpolate-lab` operator.
    Lab,
    /// Hue, chroma and luminance, the `interpolate-hcl` operator.
    Hcl,
}

impl Color {
    /// Blends two colours, as GL JS `Color.interpolate` does.
    pub fn interpolate(from: Color, to: Color, t: f64, space: ColorSpace) -> Color {
        match space {
            ColorSpace::Rgb => {
                let [r, g, b, a] = interpolate_array(from.straight(), to.straight(), t);
                Color::new(r, g, b, a)
            }
            ColorSpace::Lab => {
                let [r, g, b, a] = lab_to_rgb(interpolate_array(
                    rgb_to_lab(from.straight()),
                    rgb_to_lab(to.straight()),
                    t,
                ));
                Color::new(r, g, b, a)
            }
            ColorSpace::Hcl => {
                let [hue0, chroma0, light0, alpha0] = rgb_to_hcl(from.straight());
                let [hue1, chroma1, light1, alpha1] = rgb_to_hcl(to.straight());
                let (hue, chroma) =
                    interpolate_hcl_hue(hue0, chroma0, light0, hue1, chroma1, light1, t);
                let [r, g, b, a] = hcl_to_rgb([
                    hue,
                    chroma.unwrap_or_else(|| interpolate_number(chroma0, chroma1, t)),
                    interpolate_number(light0, light1, t),
                    interpolate_number(alpha0, alpha1, t),
                ]);
                Color::new(r, g, b, a)
            }
        }
    }
}

fn interpolate_hcl_hue(
    hue0: f64,
    chroma0: f64,
    light0: f64,
    hue1: f64,
    chroma1: f64,
    light1: f64,
    t: f64,
) -> (f64, Option<f64>) {
    if !hue0.is_nan() && !hue1.is_nan() {
        let mut delta = hue1 - hue0;
        if hue1 > hue0 && delta > 180.0 {
            delta -= 360.0;
        } else if hue1 < hue0 && hue0 - hue1 > 180.0 {
            delta += 360.0;
        }
        (hue0 + t * delta, None)
    } else if !hue0.is_nan() {
        let chroma = (light1 == 1.0 || light1 == 0.0).then_some(chroma0);
        (hue0, chroma)
    } else if !hue1.is_nan() {
        let chroma = (light0 == 1.0 || light0 == 0.0).then_some(chroma1);
        (hue1, chroma)
    } else {
        (f64::NAN, None)
    }
}

fn interpolate_array(from: [f64; 4], to: [f64; 4], t: f64) -> [f64; 4] {
    [
        interpolate_number(from[0], to[0], t),
        interpolate_number(from[1], to[1], t),
        interpolate_number(from[2], to[2], t),
        interpolate_number(from[3], to[3], t),
    ]
}

const XN: f64 = 0.96422;
const YN: f64 = 1.0;
const ZN: f64 = 0.82521;
const T0: f64 = 4.0 / 29.0;
const T1: f64 = 6.0 / 29.0;
const T2: f64 = 3.0 * T1 * T1;
const T3: f64 = T1 * T1 * T1;

fn rgb_to_xyz(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn xyz_to_lab(t: f64) -> f64 {
    if t > T3 {
        t.cbrt()
    } else {
        t / T2 + T0
    }
}

fn rgb_to_lab([r, g, b, alpha]: [f64; 4]) -> [f64; 4] {
    let (r, g, b) = (rgb_to_xyz(r), rgb_to_xyz(g), rgb_to_xyz(b));
    let y = xyz_to_lab((0.2225045 * r + 0.7168786 * g + 0.0606169 * b) / YN);
    let (x, z) = if r == g && g == b {
        (y, y)
    } else {
        (
            xyz_to_lab((0.4360747 * r + 0.3850649 * g + 0.1430804 * b) / XN),
            xyz_to_lab((0.0139322 * r + 0.0971045 * g + 0.7141733 * b) / ZN),
        )
    };
    let l = 116.0 * y - 16.0;
    [l.max(0.0), 500.0 * (x - y), 200.0 * (y - z), alpha]
}

fn lab_to_xyz(t: f64) -> f64 {
    if t > T1 {
        t * t * t
    } else {
        T2 * (t - T0)
    }
}

fn xyz_to_rgb(channel: f64) -> f64 {
    let channel = if channel <= 0.00304 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    channel.clamp(0.0, 1.0)
}

fn lab_to_rgb([l, a, b, alpha]: [f64; 4]) -> [f64; 4] {
    let y = (l + 16.0) / 116.0;
    let x = if a.is_nan() { y } else { y + a / 500.0 };
    let z = if b.is_nan() { y } else { y - b / 200.0 };
    let y = YN * lab_to_xyz(y);
    let x = XN * lab_to_xyz(x);
    let z = ZN * lab_to_xyz(z);
    [
        xyz_to_rgb(3.1338561 * x - 1.6168667 * y - 0.4906146 * z),
        xyz_to_rgb(-0.9787684 * x + 1.9161415 * y + 0.033454 * z),
        xyz_to_rgb(0.0719453 * x - 0.2289914 * y + 1.4052427 * z),
        alpha,
    ]
}

fn constrain_angle(angle: f64) -> f64 {
    let angle = angle % 360.0;
    if angle < 0.0 {
        angle + 360.0
    } else {
        angle
    }
}

fn rgb_to_hcl(rgb: [f64; 4]) -> [f64; 4] {
    let [l, a, b, alpha] = rgb_to_lab(rgb);
    let c = (a * a + b * b).sqrt();
    let h = if (c * 10000.0).round() != 0.0 {
        constrain_angle(b.atan2(a) * 180.0 / PI)
    } else {
        f64::NAN
    };
    [h, c, l, alpha]
}

fn hcl_to_rgb([h, c, l, alpha]: [f64; 4]) -> [f64; 4] {
    let h = if h.is_nan() { 0.0 } else { h * PI / 180.0 };
    lab_to_rgb([l, h.cos() * c, h.sin() * c, alpha])
}

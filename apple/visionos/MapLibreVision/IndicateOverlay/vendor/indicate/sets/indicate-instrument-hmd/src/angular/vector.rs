pub(super) fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|i| a[i] + b[i])
}
pub(super) fn scale(a: [f32; 3], b: f32) -> [f32; 3] {
    a.map(|v| v * b)
}
pub(super) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(super) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(super) fn normalized(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = libm::sqrtf(dot(a, a));
    (n.is_finite() && n > 1e-6).then(|| scale(a, 1.0 / n))
}
pub(super) fn direction(azimuth: f32, elevation: f32) -> [f32; 3] {
    [
        libm::sinf(azimuth) * libm::cosf(elevation),
        libm::cosf(azimuth) * libm::cosf(elevation),
        libm::sinf(elevation),
    ]
}

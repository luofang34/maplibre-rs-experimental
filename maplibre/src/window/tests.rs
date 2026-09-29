use super::*;

#[test]
fn fractional_logical_pixels_retain_nonzero_viewport_dimensions() {
    for scale in [1.0, 1.5, 2.0, 3.0, 4.0] {
        let logical = PhysicalSize::MIN.to_logical(scale);
        assert_eq!(logical.width(), 1);
        assert_eq!(logical.height(), 1);
    }
    let size = PhysicalSize {
        width: NonZeroU32::new(7).unwrap_or(NonZeroU32::MIN),
        height: NonZeroU32::new(3).unwrap_or(NonZeroU32::MIN),
    };
    assert_eq!(size.to_logical(2.0).width(), 3);
    assert_eq!(size.to_logical(2.0).height(), 1);
}

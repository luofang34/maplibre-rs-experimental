use super::*;
#[test]
fn scale_is_continuous_monotonic_symmetric_and_bounded() {
    let mut previous = -116.0;
    for rate in -20000..=20000 {
        let value = displacement(rate as f32);
        assert!(value >= previous && value.abs() <= 116.001);
        assert_eq!(value, -displacement(-rate as f32));
        previous = value;
    }
    assert_eq!(displacement(0.0), 0.0);
    assert_eq!(displacement(1000.0), 50.0);
    assert_eq!(displacement(2000.0), 78.0);
}

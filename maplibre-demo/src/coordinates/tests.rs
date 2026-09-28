use super::*;

#[test]
fn accepts_coordinates_and_rejects_malformed_input_without_panicking() {
    assert_eq!(
        parse_lat_long(" 48.1, -11.2 "),
        Ok(LatLon::new(48.1, -11.2))
    );
    for input in [
        "", "48.1", "north,11", "48,east", "48,11,12", "NaN,11", "48,inf",
    ] {
        assert!(parse_lat_long(input).is_err(), "accepted {input:?}");
    }
}

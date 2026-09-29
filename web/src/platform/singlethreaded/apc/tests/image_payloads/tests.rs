use super::*;

#[wasm_bindgen_test]
fn image_payloads_preserve_identity_pixels_and_optional_attempts() {
    for attempt in [None, Some(u64::MAX - 7)] {
        for source in [
            RasterSourceId::default(),
            RasterSourceId::new(Some("imagery".into())),
        ] {
            image_round_trip(attempt, source);
        }
    }
}

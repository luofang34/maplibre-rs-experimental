use indicate_instrument_scene::{HAlign, VAlign, nominal_text_ink_width, nominal_text_width};

use super::text_rect;

#[test]
fn centered_text_ink_starts_from_the_advance_anchored_origin() {
    let rect = text_rect(45.0, 12.5, 16.0, HAlign::Center, VAlign::Middle, 9);
    let expected_left = 45.0 - nominal_text_width(16.0, 9) / 2.0;
    let expected_right = expected_left + nominal_text_ink_width(16.0, 9);

    assert!((rect.min_x - expected_left).abs() < 1e-4);
    assert!((rect.max_x - expected_right).abs() < 1e-4);
    assert!(rect.min_x < 0.0);
}

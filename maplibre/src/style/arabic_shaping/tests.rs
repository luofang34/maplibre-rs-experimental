#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn letters_of_a_word_take_their_initial_medial_and_final_forms() {
    // ب ت ن: initial beh, medial teh, final noon.
    assert_eq!(shape("بتن"), "\u{FE91}\u{FE98}\u{FEE6}");
}

#[test]
fn a_letter_alone_is_isolated() {
    assert_eq!(shape("ب"), "\u{FE8F}");
    assert_eq!(shape("ء"), "\u{FE80}");
}

#[test]
fn a_right_joining_letter_does_not_join_the_next_letter() {
    // ا ب: isolated alef, then an isolated beh.
    assert_eq!(shape("اب"), "\u{FE8D}\u{FE8F}");
    // ب ا: initial beh, final alef.
    assert_eq!(shape("با"), "\u{FE91}\u{FE8E}");
}

#[test]
fn a_lam_and_an_alef_make_a_ligature() {
    assert_eq!(shape("لا"), "\u{FEFB}");
    assert_eq!(shape("بلا"), "\u{FE91}\u{FEFC}");
}

#[test]
fn other_text_is_left_alone() {
    assert_eq!(shape("Latin ボ 12"), "Latin ボ 12");
    assert!(!needs_shaping("Latin"));
    assert!(needs_shaping("a ب"));
}

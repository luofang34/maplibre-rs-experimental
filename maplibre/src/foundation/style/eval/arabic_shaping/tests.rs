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
fn a_lam_and_an_alef_make_a_ligature_even_across_diacritics() {
    assert_eq!(shape("لا"), "\u{FEFB}");
    assert_eq!(shape("بلا"), "\u{FE91}\u{FEFC}");
    // Lam + fatha (U+064E) + Alef -> Lam-Alef ligature + fatha preserved!
    assert_eq!(shape("ل\u{064E}ا"), "\u{FEFB}\u{064E}");
    assert_eq!(shape("بل\u{064E}ا"), "\u{FE91}\u{FEFC}\u{064E}");
}

#[test]
fn persian_and_urdu_extended_letters_and_tatweel_join_contextually() {
    // Persian "پچگکی": Peh initial (FB58), Tcheh medial (FB7D), Gaf medial (FB95),
    // Keheh medial (FB91), Farsi Yeh final (FBFD).
    assert_eq!(shape("پچگکی"), "\u{FB58}\u{FB7D}\u{FB95}\u{FB91}\u{FBFD}");
    // Urdu right-joining ٹ + ڈ + ڑ + ژ + ے:
    // Tteh initial (FB68) + Ddal final (FB89), Rreh isolated (FB8C), Jeh isolated (FB8A), Yeh Barree isolated (FBAE).
    assert_eq!(shape("ٹڈڑژے"), "\u{FB68}\u{FB89}\u{FB8C}\u{FB8A}\u{FBAE}");
    // Tatweel (U+0640) joins dual letters on both sides; ZWNJ (U+200C) breaks the join.
    assert_eq!(shape("بـت"), "\u{FE91}\u{0640}\u{FE96}");
    assert_eq!(shape("ب\u{200C}ت"), "\u{FE8F}\u{200C}\u{FE95}");
}

#[test]
fn other_text_is_left_alone() {
    assert_eq!(shape("Latin ボ 12"), "Latin ボ 12");
    assert!(!needs_shaping("Latin"));
    assert!(needs_shaping("a ب"));
    assert!(needs_shaping("پارس"));
}

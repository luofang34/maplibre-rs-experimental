#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn reordered(text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let mut tags: Vec<usize> = (0..chars.len()).collect();
    let line = 0..chars.len();
    reorder_lines(&mut chars, &mut tags, &[line]);
    chars.into_iter().collect()
}

#[test]
fn left_to_right_text_keeps_its_order() {
    assert_eq!(reordered("abc ボ"), "abc ボ");
}

#[test]
fn a_hebrew_word_reads_from_the_right() {
    assert_eq!(reordered("אבג"), "גבא");
}

#[test]
fn an_embedded_run_is_reversed_alone() {
    assert_eq!(reordered("ab אבג cd"), "ab גבא cd");
}

#[test]
fn a_text_that_starts_right_to_left_puts_its_later_runs_on_the_left() {
    assert_eq!(reordered("אב cd"), "cd בא");
}

#[test]
fn arabic_with_its_digits_reads_letters_from_the_right_and_digits_from_the_left() {
    // GL JS draws سلام۳۹ (Persian digits after the word) with the digits on the left, in
    // their own left-to-right order, and the joined letters after them right to left.
    let shaped = crate::style::arabic_shaping::shape("سلام۳۹");
    let letters: Vec<char> = shaped.chars().filter(|c| !c.is_numeric()).collect();
    let drawn: Vec<char> = reordered(&shaped).chars().collect();
    assert_eq!(drawn.get(..2), Some(&['۳', '۹'][..]));
    let expected_letters: Vec<char> = letters.iter().rev().copied().collect();
    assert_eq!(drawn.get(2..), Some(expected_letters.as_slice()));
    assert!(
        letters
            .iter()
            .all(|c| ('\u{FB50}'..='\u{FEFF}').contains(c)),
        "the letters are drawn in their joined presentation forms: {letters:?}"
    );
}

#[test]
fn a_paragraph_separator_ends_its_line() {
    let chars: Vec<char> = "Maktabat\u{1c} al-Iskandar".chars().collect();
    let lines = split_paragraphs(&chars, vec![0..13, 13..chars.len()]);
    assert_eq!(lines, [0..9, 9..13, 13..chars.len()]);
    let unbroken: Vec<char> = "a b".chars().collect();
    let whole: Vec<std::ops::Range<usize>> = std::iter::once(0..unbroken.len()).collect();
    assert_eq!(split_paragraphs(&unbroken, whole.clone()), whole);
}

#[test]
fn bidirectional_controls_are_not_text() {
    for control in ['\u{200E}', '\u{200F}', '\u{061C}', '\u{202B}', '\u{2067}'] {
        assert!(is_bidi_control(control), "{control:?}");
    }
    for text in ['a', ' ', 'م', '\u{1c}'] {
        assert!(!is_bidi_control(text), "{text:?}");
    }
}

#[test]
fn bidi_controls_guide_reordering_before_being_stripped() {
    // RLO (U+202E) + "abc" + PDF (U+202C) must reverse "abc" -> "cba" before stripping controls.
    let mut chars: Vec<char> = "\u{202E}abc\u{202C}".chars().collect();
    let mut tags: Vec<usize> = (0..chars.len()).collect();
    let lines = reorder_and_strip_controls(
        &mut chars,
        &mut tags,
        vec![std::ops::Range { start: 0, end: 5 }],
    );
    assert_eq!(chars.iter().collect::<String>(), "cba");
    assert_eq!(tags, [3, 2, 1]);
    assert_eq!(lines, [std::ops::Range { start: 0, end: 3 }]);
}

#[test]
fn rtl_runs_mirror_paired_brackets_per_uax9_l4() {
    // Hebrew letters wrapping "(abc)" in RTL context mirror parentheses at odd level.
    assert_eq!(reordered("א(ב)ג"), "ג(ב)א");
    assert_eq!(reordered("א[ב]ג"), "ג[ב]א");
    assert_eq!(reordered("א(12)ב"), "ב(12)א");
}

#[test]
fn combining_marks_stay_after_their_base_when_rtl_reverses() {
    // Arabic Ba + Fatha ('\u{064E}'), then Ta ('\u{062A}'): visual order is [Ta, Ba, Fatha].
    let text = "\u{0628}\u{064E}\u{062A}";
    assert_eq!(reordered(text), "\u{062A}\u{0628}\u{064E}");
}

#[test]
fn paragraph_base_level_persists_across_wrapped_lines() {
    // RTL paragraph wrapped between two LTR runs keeps paragraph base level across line break.
    let text = "אב ab cd";
    let mut chars: Vec<char> = text.chars().collect();
    let mut tags: Vec<usize> = (0..chars.len()).collect();
    let wrapped = vec![0..5, 6..chars.len()];
    reorder_lines(&mut chars, &mut tags, &wrapped);
    // Line 1 ("אב ab") reads RTL as "ab בא"; line 2 ("cd") keeps LTR order within its run.
    let line1: String = chars.get(0..5).unwrap_or(&[]).iter().collect();
    let line2: String = chars.get(6..8).unwrap_or(&[]).iter().collect();
    assert_eq!(line1, "ab בא");
    assert_eq!(line2, "cd");

    // Soft-wrapped isolate across lines continues active override across the wrap boundary.
    let iso = "\u{202E}abcd\u{202C}";
    let mut ic: Vec<char> = iso.chars().collect();
    let mut it: Vec<usize> = (0..ic.len()).collect();
    let out_lines = reorder_and_strip_controls(&mut ic, &mut it, vec![0..3, 3..6]);
    assert_eq!(out_lines, [0..2, 2..4]);
    assert_eq!(ic.iter().collect::<String>(), "badc");
}

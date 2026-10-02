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
    assert_eq!(drawn[..2], ['۳', '۹']);
    assert_eq!(
        drawn[2..],
        letters.iter().rev().copied().collect::<Vec<_>>()[..]
    );
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

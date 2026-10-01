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

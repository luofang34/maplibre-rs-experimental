use super::wrap;

/// The lines of `text` where every letter is 10 wide and a space 5.
fn broken(text: &str, max_width: f32) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let advance = |index: usize| if chars[index] == ' ' { 5.0 } else { 10.0 };
    wrap(&chars, max_width, &advance)
        .into_iter()
        .map(|line| chars[line].iter().collect())
        .collect()
}

#[test]
fn text_breaks_where_the_lines_come_out_closest_in_width() {
    // 150 wide over a 100 limit is two lines aiming for 75 each: breaking after the first word
    // leaves 40 and 100, closer than any other way to split.
    assert_eq!(broken("aaaa bbbbbbbb cc", 100.0), ["aaaa", "bbbbbbbb cc"]);
    assert_eq!(
        broken("aa bb cc dd", 1000.0),
        ["aa bb cc dd"],
        "a wide limit keeps one line"
    );
}

#[test]
fn a_newline_always_breaks_and_a_zero_width_puts_each_word_on_its_own_line() {
    assert_eq!(broken("aa\nbb", 1000.0), ["aa", "bb"]);
    assert_eq!(broken("aa bb cc", 0.0), ["aa", "bb", "cc"]);
}

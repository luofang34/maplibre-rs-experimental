use super::*;

fn collation(case_sensitive: bool, diacritic_sensitive: bool) -> Collation {
    Collation {
        case_sensitive,
        diacritic_sensitive,
        locale: None,
    }
}

#[test]
fn insensitive_comparison_ignores_case_and_accents() {
    assert_eq!(collation(false, false).compare("a", "Â"), Ordering::Equal);
    assert_eq!(collation(false, false).compare("a", "b"), Ordering::Less);
}

#[test]
fn case_alone_separates_letters_when_it_is_significant() {
    assert_eq!(collation(true, false).compare("a", "A"), Ordering::Less);
    assert_eq!(collation(true, false).compare("a", "Â"), Ordering::Less);
    assert_eq!(collation(false, false).compare("a", "A"), Ordering::Equal);
}

#[test]
fn accents_sort_after_the_plain_letter_but_before_the_next_one() {
    let collation = collation(true, true);
    assert_eq!(collation.compare("a", "\u{e4}"), Ordering::Less);
    assert_eq!(collation.compare("\u{e4}", "b"), Ordering::Less);
}

#[test]
fn scripts_that_need_shaping_are_not_supported() {
    assert!(is_supported_script("Hello, \u{4e16}\u{754c}"));
    assert!(!is_supported_script("\u{926}\u{947}\u{935}"));
    assert!(is_supported_script("\u{633}\u{644}\u{627}\u{645}"));
}

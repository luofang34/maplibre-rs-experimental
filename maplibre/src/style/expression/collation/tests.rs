use super::*;

fn collation(case_sensitive: bool, diacritic_sensitive: bool) -> Collation {
    Collation {
        case_sensitive,
        diacritic_sensitive,
        locale: None,
    }
}

fn locale_collation(locale: &str, case_sensitive: bool, diacritic_sensitive: bool) -> Collation {
    Collation {
        case_sensitive,
        diacritic_sensitive,
        locale: Some(locale.to_owned()),
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
    assert_eq!(collation(true, false).compare("e", "é"), Ordering::Equal);
    assert_eq!(collation(false, false).compare("a", "A"), Ordering::Equal);
}

#[test]
fn accent_alone_separates_diacritics_while_ignoring_case() {
    assert_eq!(collation(false, true).compare("a", "A"), Ordering::Equal);
    assert_eq!(collation(false, true).compare("e", "é"), Ordering::Less);
}

#[test]
fn accents_sort_after_the_plain_letter_but_before_the_next_one() {
    let collation = collation(true, true);
    assert_eq!(collation.compare("a", "\u{e4}"), Ordering::Less);
    assert_eq!(collation.compare("\u{e4}", "b"), Ordering::Less);
}

#[test]
fn locale_tailoring_reorders_swedish_spanish_and_german_letters() {
    // Swedish treats 'ä' as a distinct primary letter after 'z', unlike German/English.
    assert_eq!(
        locale_collation("sv-SE", false, false).compare("ä", "z"),
        Ordering::Greater
    );
    assert_eq!(
        locale_collation("de-DE", false, false).compare("ä", "b"),
        Ordering::Less
    );
    // Spanish treats 'ñ' as a distinct primary letter between 'n' and 'o'.
    assert_eq!(
        locale_collation("es", false, false).compare("n", "ñ"),
        Ordering::Less
    );
    assert_eq!(
        locale_collation("es", false, false).compare("ñ", "o"),
        Ordering::Less
    );
    assert_eq!(
        locale_collation("en", false, false).compare("n", "ñ"),
        Ordering::Equal
    );
}

#[test]
fn resolved_locale_canonicalizes_bcp47_and_falls_back_on_invalid_tags() {
    assert_eq!(collation(false, false).resolved_locale(), "en");
    assert_eq!(
        locale_collation("SV-se", false, false).resolved_locale(),
        "sv-SE"
    );
    assert_eq!(
        locale_collation("not_a_bcp47_tag!", false, false).resolved_locale(),
        "en"
    );
    assert_eq!(locale_collation("", false, false).resolved_locale(), "en");
}

#[test]
fn scripts_that_need_shaping_are_not_supported() {
    assert!(is_supported_script("Hello, \u{4e16}\u{754c}"));
    assert!(!is_supported_script("\u{926}\u{947}\u{935}"));
    assert!(is_supported_script("\u{633}\u{644}\u{627}\u{645}"));
}

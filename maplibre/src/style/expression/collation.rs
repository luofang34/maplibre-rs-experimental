//! The `collator` value: string comparison that can ignore case or diacritics, and the script
//! check behind `is-supported-script`.

use std::cmp::Ordering;

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// How two strings are compared, as `Intl.Collator` sensitivities do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collation {
    /// Whether letters that differ only in case are different.
    pub case_sensitive: bool,
    /// Whether letters that differ only in their accents are different.
    pub diacritic_sensitive: bool,
    /// The locale the strings are compared in, if the style names one.
    pub locale: Option<String>,
}

impl Collation {
    /// The locale the collation resolves to; one the style does not name is English.
    pub fn resolved_locale(&self) -> &str {
        self.locale.as_deref().unwrap_or("en")
    }

    /// Orders two strings. Base letters decide first, then accents and then case when they are
    /// significant, with lowercase before uppercase.
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        let (base_a, accents_a) = split_accents(a);
        let (base_b, accents_b) = split_accents(b);
        let lowered = |text: &str| {
            text.chars()
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        lowered(&base_a)
            .cmp(&lowered(&base_b))
            .then_with(|| {
                if self.diacritic_sensitive {
                    accents_a.cmp(&accents_b)
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| {
                if self.case_sensitive {
                    case_order(&base_a, &base_b)
                } else {
                    Ordering::Equal
                }
            })
    }
}

/// The text without its combining marks, and the marks with the position of their letter.
fn split_accents(text: &str) -> (String, Vec<(usize, char)>) {
    let mut base = String::new();
    let mut accents = Vec::new();
    for character in text.nfd() {
        if is_combining_mark(character) {
            accents.push((base.chars().count(), character));
        } else {
            base.push(character);
        }
    }
    (base, accents)
}

fn case_order(a: &str, b: &str) -> Ordering {
    for (left, right) in a.chars().zip(b.chars()) {
        match (left.is_uppercase(), right.is_uppercase()) {
            (false, true) => return Ordering::Less,
            (true, false) => return Ordering::Greater,
            _ => {}
        }
    }
    Ordering::Equal
}

/// Blocks of the scripts that need shaping the renderer does not do, as GL JS lists them; the
/// right-to-left scripts count as supported, like GL JS with its right-to-left text plugin.
const UNSUPPORTED_SCRIPTS: &[(u32, u32)] = &[
    (0x0900, 0x0DFF),
    (0x0F00, 0x0FFF),
    (0x1000, 0x109F),
    (0x1780, 0x17FF),
    (0x19E0, 0x19FF),
    (0x1CD0, 0x1CFF),
    (0xA8E0, 0xA8FF),
    (0xA9E0, 0xA9FF),
    (0xAA60, 0xAA7F),
];

/// Whether every character of `text` is in a script the renderer can lay out.
pub fn is_supported_script(text: &str) -> bool {
    text.chars().all(|character| {
        let code = u32::from(character);
        !UNSUPPORTED_SCRIPTS
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&code))
    })
}

#[cfg(test)]
mod tests;

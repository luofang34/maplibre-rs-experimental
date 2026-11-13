//! The `collator` value: locale-aware string comparison via ICU4X, and the script check behind
//! `is-supported-script`.

use std::cmp::Ordering;

use icu_collator::{
    options::{CaseLevel, CollatorOptions, Strength},
    CollatorBorrowed,
};
use icu_locale_core::{locale, Locale};

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
    /// The canonical BCP-47 locale tag the collation resolves to; unset or invalid tags fall back
    /// to `"en"`.
    pub fn resolved_locale(&self) -> String {
        self.parsed_locale().to_string()
    }

    /// Orders two strings using ICU4X compiled collation data and ECMA-402 sensitivity rules.
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        let locale = self.parsed_locale();
        let options = self.collator_options();
        match CollatorBorrowed::try_new((&locale).into(), options)
            .or_else(|_| CollatorBorrowed::try_new(Default::default(), options))
        {
            Ok(collator) => collator.compare(a, b),
            Err(_) => a.cmp(b),
        }
    }

    fn parsed_locale(&self) -> Locale {
        self.locale
            .as_deref()
            .and_then(|tag| Locale::try_from_str(tag.trim()).ok())
            .filter(|locale| !locale.id.language.is_unknown())
            .unwrap_or(locale!("en"))
    }

    fn collator_options(&self) -> CollatorOptions {
        let mut options = CollatorOptions::default();
        match (self.case_sensitive, self.diacritic_sensitive) {
            (false, false) => {
                options.strength = Some(Strength::Primary);
                options.case_level = Some(CaseLevel::Off);
            }
            (false, true) => {
                options.strength = Some(Strength::Secondary);
                options.case_level = Some(CaseLevel::Off);
            }
            (true, false) => {
                options.strength = Some(Strength::Primary);
                options.case_level = Some(CaseLevel::On);
            }
            (true, true) => {
                options.strength = Some(Strength::Tertiary);
                options.case_level = Some(CaseLevel::Off);
            }
        }
        options
    }
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

//! The `collator` value: locale-aware string comparison via ICU4X natively and `Intl.Collator`
//! on `wasm32`, plus the script check behind `is-supported-script`.

use std::cmp::Ordering;

#[cfg(not(target_arch = "wasm32"))]
use icu_collator::{
    options::{CaseLevel, CollatorOptions, Strength},
    CollatorBorrowed,
};
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
fn canonicalize_bcp47(raw: &str) -> Option<String> {
    use wasm_bindgen::{JsCast, JsValue};
    let intl = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("Intl")).ok()?;
    let func = js_sys::Reflect::get(&intl, &JsValue::from_str("getCanonicalLocales"))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let arr = func
        .call1(&intl, &JsValue::from_str(raw))
        .ok()?
        .dyn_into::<js_sys::Array>()
        .ok()?;
    let first = arr.get(0).as_string()?;
    (!first.starts_with("und")).then_some(first)
}

#[cfg(target_arch = "wasm32")]
impl Collation {
    /// The canonical BCP-47 locale tag the collation resolves to; unset or invalid tags fall back
    /// to `"en"`.
    pub fn resolved_locale(&self) -> String {
        self.locale
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(canonicalize_bcp47)
            .unwrap_or_else(|| "en".to_owned())
    }

    /// Orders two strings using the browser's built-in ECMA-402 `Intl.Collator`.
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        use wasm_bindgen::{JsCast, JsValue};
        let loc = self.resolved_locale();
        let sensitivity = match (self.case_sensitive, self.diacritic_sensitive) {
            (false, false) => "base",
            (false, true) => "accent",
            (true, false) => "case",
            (true, true) => "variant",
        };
        let opts = js_sys::Object::new();
        drop(js_sys::Reflect::set(
            &opts,
            &JsValue::from_str("sensitivity"),
            &JsValue::from_str(sensitivity),
        ));
        let locales = js_sys::Array::of1(&JsValue::from_str(&loc));
        let cmp_res = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("Intl"))
            .and_then(|intl| js_sys::Reflect::get(&intl, &JsValue::from_str("Collator")))
            .and_then(|ctor| ctor.dyn_into::<js_sys::Function>())
            .and_then(|ctor| {
                js_sys::Reflect::construct(&ctor, &js_sys::Array::of2(&locales, &opts))
            })
            .and_then(|collator| js_sys::Reflect::get(&collator, &JsValue::from_str("compare")))
            .and_then(|cmp| cmp.dyn_into::<js_sys::Function>())
            .and_then(|cmp| {
                cmp.call2(&JsValue::NULL, &JsValue::from_str(a), &JsValue::from_str(b))
            });
        match cmp_res.ok().and_then(|v| v.as_f64()) {
            Some(diff) if diff < 0.0 => Ordering::Less,
            Some(diff) if diff > 0.0 => Ordering::Greater,
            Some(_) => Ordering::Equal,
            None => a.cmp(b),
        }
    }
}

/// Scripts lacking shaping support (empty: complex Indic, Tibetan, Myanmar, Khmer, and RTL
/// scripts are shaped via `harfrust` / browser shaping and `arabic_shaping`).
const UNSUPPORTED_SCRIPTS: &[(u32, u32)] = &[];

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

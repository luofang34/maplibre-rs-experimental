//! `number-format`: locale-aware decimal and currency formatting via ICU4X.

use fixed_decimal::{
    Decimal, FloatPrecision, Sign, SignDisplay, SignedRoundingMode, UnsignedRoundingMode,
};
use icu_decimal::{options::DecimalFormatterOptions, DecimalFormatter};
use icu_locale_core::{locale, Locale};

/// Formats `number` using ICU4X locale conventions (`locale`, defaulting to `"en-US"`), digit
/// grouping, between `min` and `max` fraction digits, and currency placement when `currency` names
/// an ISO 4217 code.
pub(super) fn format(
    number: f64,
    locale_tag: Option<&str>,
    currency: Option<&str>,
    min: Option<usize>,
    max: Option<usize>,
) -> String {
    if !number.is_finite() {
        return if number.is_nan() {
            "NaN"
        } else if number > 0.0 {
            "∞"
        } else {
            "-∞"
        }
        .to_owned();
    }

    let locale = parse_locale(locale_tag);
    let currency = currency.map(str::trim).filter(|code| !code.is_empty());
    let currency_code = currency.map(str::to_ascii_uppercase);
    let currency_digits = currency_code.as_deref().map(default_currency_digits);
    let min_digits = min.or(currency_digits).unwrap_or(0).min(20);
    let max_digits = max.or(currency_digits).unwrap_or(3).max(min_digits).min(20);

    let formatter = DecimalFormatter::try_new((&locale).into(), DecimalFormatterOptions::default())
        .or_else(|_| {
            DecimalFormatter::try_new(Default::default(), DecimalFormatterOptions::default())
        });

    let Ok(mut decimal) = Decimal::try_from_f64(number, FloatPrecision::RoundTrip) else {
        return format!("{number:.max_digits$}");
    };
    decimal.round_with_mode(
        -(max_digits as i16),
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
    decimal.trim_end();
    decimal.pad_end(-(min_digits as i16));
    decimal.apply_sign_display(SignDisplay::Negative);

    let Some(code) = currency_code.as_deref() else {
        return match formatter {
            Ok(fmt) => fmt.format_to_string(&decimal),
            Err(_) => decimal.to_string(),
        };
    };

    format_currency(decimal, code, &locale, formatter.as_ref().ok())
}

fn parse_locale(tag: Option<&str>) -> Locale {
    tag.and_then(|raw| Locale::try_from_str(raw.trim()).ok())
        .filter(|loc| !loc.id.language.is_unknown())
        .unwrap_or(locale!("en-US"))
}

fn default_currency_digits(code: &str) -> usize {
    match code {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

struct CurrencyUnit<'a> {
    text: &'a str,
    is_iso_code: bool,
}

fn currency_unit<'a>(code: &'a str, locale: &Locale) -> CurrencyUnit<'a> {
    let lang = locale.id.language.as_str();
    let region = locale.id.region.as_ref().map(|r| r.as_str());
    let symbol = match code {
        "USD" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "JPY" => Some("¥"),
        "CNY" => Some(if lang == "zh" { "¥" } else { "CN¥" }),
        "INR" => Some("₹"),
        "KRW" => Some("₩"),
        "RUB" => Some("₽"),
        "TRY" => Some("₺"),
        "ILS" => Some("₪"),
        "THB" => Some("฿"),
        "VND" => Some("₫"),
        "BRL" => Some("R$"),
        "HKD" => Some("HK$"),
        "TWD" => Some("NT$"),
        "CAD" => Some(if region == Some("CA") { "$" } else { "CA$" }),
        "AUD" => Some(if region == Some("AU") { "$" } else { "A$" }),
        "NZD" => Some(if region == Some("NZ") { "$" } else { "NZ$" }),
        "MXN" => Some(if region == Some("MX") { "$" } else { "MX$" }),
        _ => None,
    };
    match symbol {
        Some(sym) => CurrencyUnit {
            text: sym,
            is_iso_code: false,
        },
        None => CurrencyUnit {
            text: code,
            is_iso_code: true,
        },
    }
}

fn uses_currency_suffix(locale: &Locale) -> bool {
    let lang = locale.id.language.as_str();
    let region = locale.id.region.as_ref().map(|r| r.as_str());
    match lang {
        "de" | "fr" | "fi" | "sv" | "da" | "nb" | "nn" | "no" | "pl" | "cs" | "sk" | "hu"
        | "ro" | "bg" | "hr" | "sl" | "sr" | "ru" | "uk" | "lt" | "lv" | "et" | "el" | "vi" => true,
        "es" => matches!(region, None | Some("ES")),
        _ => false,
    }
}

fn format_currency(
    decimal: Decimal,
    code: &str,
    locale: &Locale,
    formatter: Option<&DecimalFormatter>,
) -> String {
    let unit = currency_unit(code, locale);
    if uses_currency_suffix(locale) {
        let signed = match formatter {
            Some(fmt) => fmt.format_to_string(&decimal),
            None => decimal.to_string(),
        };
        return format!("{signed}\u{a0}{}", unit.text);
    }

    let is_negative = decimal.sign == Sign::Negative;
    let mut unsigned = decimal;
    unsigned.sign = Sign::None;
    let digits = match formatter {
        Some(fmt) => fmt.format_to_string(&unsigned),
        None => unsigned.to_string(),
    };
    let sign = if is_negative { "-" } else { "" };
    if unit.is_iso_code {
        format!("{sign}{}\u{a0}{digits}", unit.text)
    } else {
        format!("{sign}{}{digits}", unit.text)
    }
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn groups_digits_and_trims_trailing_zeros() {
        assert_eq!(format(1234567.891, None, None, None, None), "1,234,567.891");
        assert_eq!(format(0.5, None, None, None, None), "0.5");
        assert_eq!(format(2.0, None, None, Some(2), None), "2.00");
        assert_eq!(format(110.5744, None, None, None, Some(3)), "110.574");
        assert_eq!(format(-0.0, None, None, None, None), "0");
        assert_eq!(format(f64::NAN, None, None, None, None), "NaN");
        assert_eq!(format(f64::INFINITY, None, None, None, None), "∞");
        assert_eq!(format(f64::NEG_INFINITY, None, None, None, None), "-∞");
    }

    #[test]
    fn formats_decimals_across_locales_and_numbering_systems() {
        assert_eq!(
            format(1234567.89, Some("en-US"), None, None, None),
            "1,234,567.89"
        );
        assert_eq!(
            format(1234567.89, Some("de-DE"), None, None, None),
            "1.234.567,89"
        );
        assert_eq!(
            format(1234567.89, Some("fr-FR"), None, None, None),
            "1\u{202f}234\u{202f}567,89"
        );
        assert_eq!(
            format(1234567.89, Some("ja-JP"), None, None, None),
            "1,234,567.89"
        );
        assert_eq!(
            format(1234567.89, Some("hi-IN"), None, None, None),
            "12,34,567.89"
        );
        assert_eq!(format(1234.56, Some("ar-EG"), None, None, None), "١٬٢٣٤٫٥٦");
        assert_eq!(
            format(1234.56, Some("invalid_locale!"), None, None, None),
            "1,234.56"
        );
    }

    #[test]
    fn formats_currencies_with_locale_conventions_and_iso_minor_units() {
        assert_eq!(format(1234.5, None, Some("USD"), None, None), "$1,234.50");
        assert_eq!(format(-3.0, None, Some("EUR"), None, None), "-€3.00");
        assert_eq!(
            format(1234.5, Some("de-DE"), Some("EUR"), None, None),
            "1.234,50\u{a0}€"
        );
        assert_eq!(
            format(-3.0, Some("de-DE"), Some("EUR"), None, None),
            "-3,00\u{a0}€"
        );
        assert_eq!(
            format(1234.5, Some("fr-FR"), Some("EUR"), None, None),
            "1\u{202f}234,50\u{a0}€"
        );
        assert_eq!(
            format(1234.5, Some("ja-JP"), Some("JPY"), None, None),
            "¥1,235"
        );
        assert_eq!(
            format(1234567.5, Some("hi-IN"), Some("INR"), None, None),
            "₹12,34,567.50"
        );
        assert_eq!(
            format(1234.5, Some("en-US"), Some("CHF"), None, None),
            "CHF\u{a0}1,234.50"
        );
    }
}

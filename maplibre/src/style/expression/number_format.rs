//! `number-format`: a number written the way an English locale writes it.

/// Formats `number` with digit grouping, between `min` and `max` fraction digits, and a
/// currency symbol when `currency` names one.
pub(super) fn format(
    number: f64,
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
    let currency_digits = currency.map(|_| 2);
    let min_digits = min.or(currency_digits).unwrap_or(0);
    let max_digits = max.or(currency_digits).unwrap_or(3).max(min_digits);
    let fixed = format!("{:.*}", max_digits, number.abs());
    let (whole, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let mut fraction = fraction.to_owned();
    while fraction.len() > min_digits && fraction.ends_with('0') {
        fraction.pop();
    }
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let mut text = grouped;
    if !fraction.is_empty() {
        text.push('.');
        text.push_str(&fraction);
    }
    let negative = number < 0.0 && text.chars().any(|c| c.is_ascii_digit() && c != '0');
    let sign = if negative { "-" } else { "" };
    match currency {
        Some(code) => format!("{sign}{}{text}", symbol(code)),
        None => format!("{sign}{text}"),
    }
}

fn symbol(code: &str) -> String {
    match code {
        "USD" => "$".to_owned(),
        "EUR" => "€".to_owned(),
        "GBP" => "£".to_owned(),
        "JPY" => "¥".to_owned(),
        other => format!("{other}\u{a0}"),
    }
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn groups_digits_and_trims_trailing_zeros() {
        assert_eq!(format(1234567.891, None, None, None), "1,234,567.891");
        assert_eq!(format(0.5, None, None, None), "0.5");
        assert_eq!(format(2.0, None, Some(2), None), "2.00");
        assert_eq!(format(110.5744, None, None, Some(3)), "110.574");
    }

    #[test]
    fn a_currency_has_two_fraction_digits_and_a_symbol() {
        assert_eq!(format(1234.5, Some("USD"), None, None), "$1,234.50");
        assert_eq!(format(-3.0, Some("EUR"), None, None), "-€3.00");
    }
}

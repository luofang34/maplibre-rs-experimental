//! Style `font-faces` specification types, CSS `unicode-range` parsing, and font-stack matching.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Default value for `Style::local_ideograph_font_family` (`Some("sans-serif")`).
pub fn default_local_ideograph_font_family() -> Option<String> {
    Some("sans-serif".to_owned())
}

/// Returns `true` when `value` matches [`default_local_ideograph_font_family`], used by Serde
/// `skip_serializing_if` so styles without an explicit override serialize cleanly.
pub fn is_default_local_ideograph_font_family(value: &Option<String>) -> bool {
    value.as_deref() == Some("sans-serif")
}

/// Deserializes `localIdeographFontFamily`, accepting a font family string, `false`, or `null`.
pub fn deserialize_local_ideograph_font_family<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false)) => Ok(None),
        Some(serde_json::Value::Bool(true)) => Ok(default_local_ideograph_font_family()),
        Some(serde_json::Value::String(family)) => {
            let trimmed = family.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or boolean for localIdeographFontFamily, got {other}"
        ))),
    }
}

/// Font weight (`100..=900` or CSS keywords `normal` / `bold`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FontWeight(pub u16);

impl Default for FontWeight {
    fn default() -> Self {
        Self(400)
    }
}

impl<'de> Deserialize<'de> for FontWeight {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match &value {
            serde_json::Value::Number(num) => {
                let raw = num.as_u64().unwrap_or(400).clamp(1, 1000) as u16;
                Ok(Self(raw))
            }
            serde_json::Value::String(text) => Ok(Self::from_css(text)),
            _ => Ok(Self::default()),
        }
    }
}

impl FontWeight {
    /// Parses a CSS weight keyword or numeric string.
    pub fn from_css(value: &str) -> Self {
        let lower = value.trim().to_ascii_lowercase();
        if let Ok(parsed) = lower.parse::<u16>() {
            return Self(parsed.clamp(1, 1000));
        }
        match lower.as_str() {
            "thin" | "hairline" => Self(100),
            "extralight" | "extra-light" | "ultralight" | "ultra-light" => Self(200),
            "light" => Self(300),
            "normal" | "regular" | "book" => Self(400),
            "medium" => Self(500),
            "semibold" | "semi-bold" | "demibold" | "demi-bold" => Self(600),
            "bold" => Self(700),
            "extrabold" | "extra-bold" | "ultrabold" | "ultra-bold" => Self(800),
            "black" | "heavy" => Self(900),
            _ => Self(400),
        }
    }
}

/// Font posture (`normal`, `italic`, or `oblique`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FontStyle {
    /// Upright roman face.
    #[default]
    Normal,
    /// Cursive italic face.
    Italic,
    /// Slanted oblique face.
    Oblique,
}

impl<'de> Deserialize<'de> for FontStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
        Ok(Self::from_css(&text))
    }
}

impl FontStyle {
    /// Parses a CSS `font-style` token.
    pub fn from_css(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "italic" => Self::Italic,
            "oblique" => Self::Oblique,
            _ => Self::Normal,
        }
    }
}

/// Closed Unicode scalar interval `[start, end]` from a CSS `unicode-range` descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnicodeRange {
    /// Inclusive starting codepoint.
    pub start: u32,
    /// Inclusive ending codepoint.
    pub end: u32,
}

impl UnicodeRange {
    /// Checks whether `codepoint` falls inside `[start, end]`.
    pub fn contains(self, codepoint: u32) -> bool {
        (self.start..=self.end).contains(&codepoint)
    }

    /// Parses a comma-separated CSS `unicode-range` list such as `"U+0000-00FF, U+09??"`.
    pub fn parse_list(input: &str) -> Vec<Self> {
        input
            .split(',')
            .filter_map(|part| Self::parse_single(part.trim()))
            .collect()
    }

    fn parse_single(token: &str) -> Option<Self> {
        let hex = token
            .strip_prefix("U+")
            .or_else(|| token.strip_prefix("u+"))?;
        if let Some((lo, hi)) = hex.split_once('-') {
            let start = u32::from_str_radix(lo.trim(), 16).ok()?;
            let end = u32::from_str_radix(hi.trim(), 16).ok()?;
            return (start <= end && end <= 0x10FFFF).then_some(Self { start, end });
        }
        if hex.contains('?') {
            let lo_str: String = hex
                .chars()
                .map(|c| if c == '?' { '0' } else { c })
                .collect();
            let hi_str: String = hex
                .chars()
                .map(|c| if c == '?' { 'F' } else { c })
                .collect();
            let start = u32::from_str_radix(&lo_str, 16).ok()?;
            let end = u32::from_str_radix(&hi_str, 16).ok()?.min(0x10FFFF);
            return (start <= end).then_some(Self { start, end });
        }
        let single = u32::from_str_radix(hex.trim(), 16).ok()?;
        (single <= 0x10FFFF).then_some(Self {
            start: single,
            end: single,
        })
    }
}

/// One `@font-face`-style font declaration in a MapLibre style's `"font-faces"` root property.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FontFaceRule {
    /// Declared family name matched by `"text-font"` entries.
    #[serde(rename = "font-family")]
    pub family: String,
    /// Candidate font URLs in priority order.
    pub urls: Vec<String>,
    /// Declared weight (defaults to `400`).
    #[serde(rename = "font-weight")]
    pub weight: FontWeight,
    /// Declared posture (defaults to `normal`).
    #[serde(rename = "font-style")]
    pub style: FontStyle,
    /// Optional Unicode coverage filter; empty means unrestricted.
    #[serde(rename = "unicode-range", skip_serializing_if = "Vec::is_empty")]
    pub unicode_ranges: Vec<UnicodeRange>,
}

impl FontFaceRule {
    /// Returns `true` when `codepoint` is covered by this rule's `unicode-range` list.
    pub fn covers(&self, codepoint: u32) -> bool {
        self.unicode_ranges.is_empty()
            || self
                .unicode_ranges
                .iter()
                .any(|range| range.contains(codepoint))
    }
}

/// Collection of `"font-faces"` declarations on a [`crate::style::Style`].
///
/// Accepts both an array of `@font-face` objects and a map keyed by family name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FontFaces(pub Vec<FontFaceRule>);

impl FontFaces {
    /// Returns `true` when no font-face rules are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for FontFaces {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let mut rules = Vec::new();
        match raw {
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(rule) = parse_rule_object(None, &item) {
                        rules.push(rule);
                    }
                }
            }
            serde_json::Value::Object(map) => {
                // Deterministic iteration order across keys.
                let ordered: BTreeMap<String, serde_json::Value> = map.into_iter().collect();
                for (family, entry) in ordered {
                    match entry {
                        serde_json::Value::String(url) => {
                            let urls = extract_urls_from_str(&url);
                            if !urls.is_empty() {
                                rules.push(FontFaceRule {
                                    family,
                                    urls,
                                    weight: FontWeight::default(),
                                    style: FontStyle::default(),
                                    unicode_ranges: Vec::new(),
                                });
                            }
                        }
                        serde_json::Value::Array(sub_items) => {
                            for sub in sub_items {
                                if let Some(rule) = parse_rule_object(Some(&family), &sub) {
                                    rules.push(rule);
                                }
                            }
                        }
                        other => {
                            if let Some(rule) = parse_rule_object(Some(&family), &other) {
                                rules.push(rule);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(Self(rules))
    }
}

fn parse_rule_object(
    default_family: Option<&str>,
    value: &serde_json::Value,
) -> Option<FontFaceRule> {
    let obj = value.as_object()?;
    let family = obj
        .get("font-family")
        .or_else(|| obj.get("fontFamily"))
        .or_else(|| obj.get("family"))
        .and_then(serde_json::Value::as_str)
        .or(default_family)?
        .trim()
        .trim_matches(['"', '\''])
        .to_owned();
    if family.is_empty() {
        return None;
    }
    let mut urls = Vec::new();
    for key in ["url", "src", "urls"] {
        if let Some(src_val) = obj.get(key) {
            collect_urls(src_val, &mut urls);
        }
    }
    if urls.is_empty() {
        return None;
    }
    let weight = obj
        .get("font-weight")
        .or_else(|| obj.get("fontWeight"))
        .or_else(|| obj.get("weight"))
        .and_then(|v| serde_json::from_value::<FontWeight>(v.clone()).ok())
        .unwrap_or_default();
    let style = obj
        .get("font-style")
        .or_else(|| obj.get("fontStyle"))
        .or_else(|| obj.get("style"))
        .and_then(|v| serde_json::from_value::<FontStyle>(v.clone()).ok())
        .unwrap_or_default();
    let unicode_ranges = obj
        .get("unicode-range")
        .or_else(|| obj.get("unicodeRange"))
        .map(parse_unicode_ranges_value)
        .unwrap_or_default();
    Some(FontFaceRule {
        family,
        urls,
        weight,
        style,
        unicode_ranges,
    })
}

fn collect_urls(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.extend(extract_urls_from_str(s)),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_urls(item, out);
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(url) = obj.get("url").and_then(serde_json::Value::as_str) {
                out.extend(extract_urls_from_str(url));
            }
        }
        _ => {}
    }
}

fn extract_urls_from_str(input: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for piece in input.split(',') {
        let trimmed = piece.trim();
        if let Some(rest) = trimmed.strip_prefix("url(") {
            let inner = rest
                .split(')')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['"', '\'']);
            if !inner.is_empty() {
                urls.push(inner.to_owned());
            }
        } else {
            let clean = trimmed.trim_matches(['"', '\'']);
            if !clean.is_empty() {
                urls.push(clean.to_owned());
            }
        }
    }
    urls
}

fn parse_unicode_ranges_value(value: &serde_json::Value) -> Vec<UnicodeRange> {
    match value {
        serde_json::Value::String(s) => UnicodeRange::parse_list(s),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .flat_map(UnicodeRange::parse_list)
            .collect(),
        _ => Vec::new(),
    }
}

/// Decomposes a MapLibre font-stack entry such as `"Open Sans Semibold Italic"` into a base
/// family name, [`FontWeight`], and [`FontStyle`].
pub fn parse_font_stack_entry(entry: &str) -> (String, FontWeight, FontStyle) {
    let trimmed = entry.trim().trim_matches(['"', '\'']);
    let mut tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let mut style = FontStyle::Normal;
    let mut weight = FontWeight::default();
    while let Some(&last) = tokens.last() {
        let lower = last.to_ascii_lowercase();
        match lower.as_str() {
            "italic" => {
                style = FontStyle::Italic;
                tokens.pop();
            }
            "oblique" => {
                style = FontStyle::Oblique;
                tokens.pop();
            }
            "regular" | "normal" | "roman" | "book" => {
                weight = FontWeight(400);
                tokens.pop();
            }
            "thin" | "hairline" => {
                weight = FontWeight(100);
                tokens.pop();
            }
            "extralight" | "ultralight" => {
                weight = FontWeight(200);
                tokens.pop();
            }
            "light" => {
                weight = FontWeight(300);
                tokens.pop();
            }
            "medium" => {
                weight = FontWeight(500);
                tokens.pop();
            }
            "semibold" | "demibold" => {
                weight = FontWeight(600);
                tokens.pop();
            }
            "bold" => {
                weight = FontWeight(700);
                tokens.pop();
            }
            "extrabold" | "ultrabold" => {
                weight = FontWeight(800);
                tokens.pop();
            }
            "black" | "heavy" => {
                weight = FontWeight(900);
                tokens.pop();
            }
            _ => break,
        }
    }
    let family = if tokens.is_empty() {
        trimmed.to_owned()
    } else {
        tokens.join(" ")
    };
    (family, weight, style)
}

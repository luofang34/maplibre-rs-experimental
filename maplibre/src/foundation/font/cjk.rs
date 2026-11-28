//! Unicode block classification for local CJK ideograph generation and complex script shaping.

/// Returns `true` when `ch` belongs to a CJK ideograph, Kana, Hangul, Bopomofo, Yi, or CJK
/// punctuation/symbol block that MapLibre rasterises locally when `localIdeographFontFamily`
/// is configured.
///
/// Covers Basic Multilingual Plane blocks as well as Supplementary Ideographic Planes (SIP/TIP,
/// `U+20000..=U+323AF`, including Extension B–I characters such as `𪚥` and `𦨭`).
pub fn is_local_cjk_codepoint(ch: char) -> bool {
    matches!(
        u32::from(ch),
        // Hangul Jamo
        0x1100..=0x11FF
            // CJK Radicals Supplement, Kangxi Radicals, Ideographic Description, CJK Symbols & Punctuation
            | 0x2E80..=0x303F
            // Hiragana & Katakana
            | 0x3040..=0x30FF
            // Bopomofo, Hangul Compatibility Jamo, Kanbun, Bopomofo Extended, CJK Strokes, Katakana Phonetic Extensions
            | 0x3100..=0x31FF
            // Enclosed CJK Letters and Months & CJK Compatibility
            | 0x3200..=0x33FF
            // CJK Unified Ideographs Extension A
            | 0x3400..=0x4DBF
            // CJK Unified Ideographs
            | 0x4E00..=0x9FFF
            // Yi Syllables & Yi Radicals
            | 0xA000..=0xA4CF
            // Hangul Jamo Extended-A
            | 0xA960..=0xA97F
            // Hangul Syllables & Hangul Jamo Extended-B
            | 0xAC00..=0xD7FF
            // CJK Compatibility Ideographs
            | 0xF900..=0xFAFF
            // Vertical Forms & CJK Compatibility Forms & Small Form Variants
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE6F
            // Halfwidth and Fullwidth Forms
            | 0xFF00..=0xFFEF
            // Kana Supplement, Kana Extended-A, Small Kana Extension
            | 0x1B000..=0x1B16F
            // Enclosed Ideographic Supplement
            | 0x1F200..=0x1F2FF
            // CJK Unified Ideographs Extensions B, C, D, E, F & CJK Compatibility Ideographs Supplement
            | 0x20000..=0x2FA1F
            // CJK Unified Ideographs Extensions G, H, I
            | 0x30000..=0x323AF
    )
}

/// Returns `true` when `ch` belongs to a script or combining-mark range whose typography
/// benefits from OpenType `GSUB`/`GPOS` shaping (Arabic, Indic, Southeast Asian, Tibetan,
/// or combining marks).
pub fn is_complex_shaping_codepoint(ch: char) -> bool {
    matches!(
        u32::from(ch),
        // Combining Diacritical Marks
        0x0300..=0x036F
            // Hebrew, Arabic, Syriac, Arabic Supplement, Thaana, NKo, Samaritan, Mandaic, Arabic Extended
            | 0x0590..=0x08FF
            // Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Telugu, Kannada, Malayalam, Sinhala
            | 0x0900..=0x0DFF
            // Thai & Lao
            | 0x0E00..=0x0EFF
            // Tibetan, Myanmar
            | 0x0F00..=0x109F
            // Tagalog, Hanunoo, Buhid, Tagbanwa, Khmer, Mongolian
            | 0x1700..=0x18AF
            // Limbu, Tai Le, New Tai Lue, Khmer Symbols, Buginese, Tai Tham
            | 0x1900..=0x1AAF
            // Balinese, Sundanese, Batak, Lepcha
            | 0x1B00..=0x1C4F
            // Vedic Extensions, Combining Diacritical Marks Extended/Supplement
            | 0x1CD0..=0x1DFF
            // Zero-width non-joiner / joiner
            | 0x200C..=0x200D
            // Syloti Nagri, Devanagari Extended, Kayah Li, Rejang, Javanese, Myanmar Extended-B, Cham, Myanmar Extended-A, Tai Viet
            | 0xA800..=0xAADF
    )
}

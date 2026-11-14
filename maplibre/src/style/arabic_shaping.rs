//! Arabic, Persian, Urdu, and extended Arabic contextual shaping into Unicode presentation forms.
//!
//! When an OpenType font face is loaded, [`crate::font`] runs full `GSUB`/`GPOS` shaping via
//! `harfrust`. When only legacy `.pbf` SDF glyph ranges are available, this module maps Unicode
//! `U+0600..U+06FF` letters into their isolated, final, initial, and medial presentation forms
//! (`U+FB50..U+FDFF` and `U+FE80..U+FEFC`), handles `U+0640` Tatweel and `ZWJ`/`ZWNJ`, and forms
//! Lam-Alef ligatures across intervening `tashkeel` diacritics.

/// Contextual joining class defined by Unicode Arabic Cursive Joining (Section 9.2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Joining {
    /// Joins to both neighbours (isolated, final, initial, medial).
    Dual,
    /// Joins only to a preceding dual-joining letter (isolated, final).
    Right,
    /// Breaks cursive joining on both sides.
    None,
}

/// Explicit `[isolated, final, initial, medial]` glyph codepoints and joining class for `c`.
fn letter_forms(c: char) -> Option<([u32; 4], Joining)> {
    let cp = u32::from(c);
    let dual = |iso: u32| ([iso, iso + 1, iso + 2, iso + 3], Joining::Dual);
    let right = |iso: u32| ([iso, iso + 1, iso, iso + 1], Joining::Right);
    let self_dual = |u: u32| ([u, u, u, u], Joining::Dual);
    let self_right = |u: u32| ([u, u, u, u], Joining::Right);

    let entry = match cp {
        // Core Arabic (U+0621..U+064A) -> Presentation Forms-B (U+FE80..U+FEF4)
        0x0621 => ([0xFE80; 4], Joining::None),
        0x0622 => right(0xFE81),
        0x0623 => right(0xFE83),
        0x0624 => right(0xFE85),
        0x0625 => right(0xFE87),
        0x0626 => dual(0xFE89),
        0x0627 => right(0xFE8D),
        0x0628 => dual(0xFE8F),
        0x0629 => right(0xFE93),
        0x062A..=0x062E => dual(0xFE95 + (cp - 0x062A) * 4),
        0x062F => right(0xFEA9),
        0x0630 => right(0xFEAB),
        0x0631 => right(0xFEAD),
        0x0632 => right(0xFEAF),
        0x0633..=0x063A => dual(0xFEB1 + (cp - 0x0633) * 4),
        // Tatweel / Kashida (U+0640) & ZWJ (U+200D) cause adjacent letters to join without changing codepoint.
        0x0640 | 0x200D => self_dual(cp),
        0x0641..=0x0646 => dual(0xFED1 + (cp - 0x0641) * 4),
        0x0647 => dual(0xFEE9),
        0x0648 => right(0xFEED),
        0x0649 => right(0xFEEF),
        0x064A => dual(0xFEF1),
        // Persian, Urdu, Kurdish, Sindhi, Uyghur, Pashto -> Presentation Forms-A (U+FB50..U+FBFF)
        0x0671 => right(0xFB50), // Alef Wasla (ٱ)
        0x0679 => dual(0xFB66),  // Tteh (ٹ)
        0x067A => dual(0xFB5E),  // Tteheh (ٺ)
        0x067B => dual(0xFB52),  // Beeh (ٻ)
        0x067E => dual(0xFB56),  // Peh (پ)
        0x067F => dual(0xFB62),  // Teheh (ٿ)
        0x0680 => dual(0xFB5A),  // Beheh (ڀ)
        0x0683 => dual(0xFB76),  // Nyeh (ڃ)
        0x0684 => dual(0xFB72),  // Dyeh (ڄ)
        0x0686 => dual(0xFB7A),  // Tcheh (چ)
        0x0687 => dual(0xFB7E),  // Tcheheh (ڇ)
        0x0688 => right(0xFB88), // Ddal (ڈ)
        0x068C => right(0xFB84), // Dahal (ڌ)
        0x068D => right(0xFB82), // Ddahal (ڍ)
        0x068E => right(0xFB86), // Dul (ڎ)
        0x0691 => right(0xFB8C), // Rreh (ڑ)
        0x0698 => right(0xFB8A), // Jeh (ژ)
        0x06A4 => dual(0xFB6A),  // Veh (ڤ)
        0x06A6 => dual(0xFB6E),  // Peheh (ڦ)
        0x06A9 => dual(0xFB8E),  // Keheh (ک)
        0x06AD => dual(0xFBD3),  // Ng (ڭ)
        0x06AF => dual(0xFB92),  // Gaf (گ)
        0x06B1 => dual(0xFB9A),  // Ngoeh (ڱ)
        0x06B3 => dual(0xFB96),  // Gueh (ڳ)
        // Urdu Noon Ghunna (ں): dotless in isolated/final, dotted Noon in initial/medial.
        0x06BA => ([0xFB9E, 0xFB9F, 0xFEE7, 0xFEE8], Joining::Dual),
        0x06BB => dual(0xFBA0),  // Rnoon (ڻ)
        0x06BE => dual(0xFBAA),  // Heh Doachashmee (ھ)
        0x06C0 => right(0xFBA4), // Heh with Yeh above (ۀ)
        0x06C1 => dual(0xFBA6),  // Heh Goal (ہ)
        0x06C5 => right(0xFBE0),
        0x06C6 => right(0xFBD9),
        0x06C7 => right(0xFBD7),
        0x06C8 => right(0xFBDB),
        0x06C9 => right(0xFBE2),
        0x06CB => right(0xFBDE),
        0x06CC => dual(0xFBFC), // Farsi Yeh (ی)
        0x06D0 => dual(0xFBE4),
        0x06D2 => right(0xFBAE), // Yeh Barree (ے)
        0x06D3 => right(0xFBB0), // Yeh Barree with Hamza above (ۓ)
        // Remaining U+0600..U+06FF Arabic letters participate in contextual joining for neighbours.
        0x0672..=0x0677 | 0x0689..=0x0699 | 0x06C2..=0x06CD | 0x06D5 | 0x06EE..=0x06EF => {
            self_right(cp)
        }
        0x063B..=0x063F
        | 0x066E..=0x066F
        | 0x0678..=0x0687
        | 0x069A..=0x06BF
        | 0x06CE..=0x06D1
        | 0x06FA..=0x06FC
        | 0x06FF => self_dual(cp),
        _ => return None,
    };
    Some(entry)
}

/// Non-spacing harakat, tashkeel, and Quranic marks that do not interrupt cursive joining.
fn is_transparent(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
    )
}

/// Isolated and final Lam-Alef ligature codepoints for an Alef character.
fn lam_alef(alef: char) -> Option<(char, char)> {
    let (isolated, joined) = match u32::from(alef) {
        0x0622 => (0xFEF5, 0xFEF6),
        0x0623 => (0xFEF7, 0xFEF8),
        0x0625 => (0xFEF9, 0xFEFA),
        0x0627 => (0xFEFB, 0xFEFC),
        _ => return None,
    };
    Some((char::from_u32(isolated)?, char::from_u32(joined)?))
}

/// Whether `text` contains any Arabic/Persian/Urdu letter that undergoes contextual shaping.
pub fn needs_shaping(text: &str) -> bool {
    text.chars()
        .any(|c| !matches!(u32::from(c), 0x0640 | 0x200D) && letter_forms(c).is_some())
}

/// Returns `text` in logical order with each Arabic/Persian/Urdu letter mapped to its contextual
/// presentation form and Lam + (optional marks) + Alef combined into a Lam-Alef ligature.
pub fn shape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let joins_before = |index: usize| -> bool {
        chars[..index]
            .iter()
            .rev()
            .find(|c| !is_transparent(**c))
            .and_then(|c| letter_forms(*c))
            .is_some_and(|(_, joining)| joining == Joining::Dual)
    };
    let joins_after = |index: usize| -> bool {
        chars[index + 1..]
            .iter()
            .find(|c| !is_transparent(**c))
            .and_then(|c| letter_forms(*c))
            .is_some_and(|(_, joining)| joining != Joining::None)
    };

    let mut shaped = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '\u{200D}' {
            index += 1;
            continue;
        }
        let Some((table, joining)) = letter_forms(c) else {
            shaped.push(c);
            index += 1;
            continue;
        };
        let before = joins_before(index);

        // Lam + optional transparent tashkeel + Alef -> Lam-Alef ligature + preserved tashkeel.
        if u32::from(c) == 0x0644 {
            let mut look = index + 1;
            while look < chars.len() && is_transparent(chars[look]) {
                look += 1;
            }
            if let Some((alone, joined)) = chars.get(look).and_then(|next| lam_alef(*next)) {
                shaped.push(if before { joined } else { alone });
                for &mark in &chars[index + 1..look] {
                    shaped.push(mark);
                }
                index = look + 1;
                continue;
            }
        }

        let slot = match (joining, before, joins_after(index)) {
            (Joining::None, ..) | (_, false, false) => 0,
            (_, true, false) => 1,
            (Joining::Dual, false, true) => 2,
            (Joining::Dual, true, true) => 3,
            (Joining::Right, false, true) => 0,
            (Joining::Right, true, true) => 1,
        };
        shaped.push(char::from_u32(table[slot]).unwrap_or(c));
        index += 1;
    }
    shaped
}

#[cfg(test)]
#[path = "arabic_shaping/tests.rs"]
mod tests;

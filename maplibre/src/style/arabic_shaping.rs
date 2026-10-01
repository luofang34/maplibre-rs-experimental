//! Arabic letters in the joined forms they take in a word, as Unicode's presentation forms.
//!
//! Text is shaped before glyphs are requested and before it is laid out, the way GL JS applies
//! its RTL plugin, so each form is a character of its own with the advance of its own glyph.

/// How a letter joins its neighbours.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Joining {
    /// Joins to both neighbours: isolated, final, initial and medial forms exist.
    Dual,
    /// Joins only to the letter before it: isolated and final forms exist.
    Right,
    /// Does not join.
    None,
}

/// The isolated form of a letter and how many forms follow it: final, initial and medial for
/// a dual-joining letter, the final alone for a right-joining one.
fn forms(c: char) -> Option<(u32, Joining)> {
    let (isolated, joining) = match u32::from(c) {
        0x0621 => (0xFE80, Joining::None),
        0x0622 => (0xFE81, Joining::Right),
        0x0623 => (0xFE83, Joining::Right),
        0x0624 => (0xFE85, Joining::Right),
        0x0625 => (0xFE87, Joining::Right),
        0x0626 => (0xFE89, Joining::Dual),
        0x0627 => (0xFE8D, Joining::Right),
        0x0628 => (0xFE8F, Joining::Dual),
        0x0629 => (0xFE93, Joining::Right),
        0x062A..=0x062E => (0xFE95 + (u32::from(c) - 0x062A) * 4, Joining::Dual),
        0x062F => (0xFEA9, Joining::Right),
        0x0630 => (0xFEAB, Joining::Right),
        0x0631 => (0xFEAD, Joining::Right),
        0x0632 => (0xFEAF, Joining::Right),
        0x0633..=0x063A => (0xFEB1 + (u32::from(c) - 0x0633) * 4, Joining::Dual),
        0x0641..=0x0646 => (0xFED1 + (u32::from(c) - 0x0641) * 4, Joining::Dual),
        0x0647 => (0xFEE9, Joining::Dual),
        0x0648 => (0xFEED, Joining::Right),
        0x0649 => (0xFEEF, Joining::Right),
        0x064A => (0xFEF1, Joining::Dual),
        _ => return None,
    };
    Some((isolated, joining))
}

/// Marks that sit on a letter without breaking the join between its neighbours.
fn is_transparent(c: char) -> bool {
    matches!(u32::from(c), 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4)
}

/// The form of the lam-alef ligature for an alef, isolated and final.
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

/// Whether `c` is a letter this module shapes.
fn is_shaped(c: char) -> bool {
    forms(c).is_some()
}

/// Whether the text has a letter to shape.
pub fn needs_shaping(text: &str) -> bool {
    text.chars().any(is_shaped)
}

/// `text` with each Arabic letter in its joined form, in logical order, and a lam followed by
/// an alef as their ligature.
pub fn shape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let joins_before = |index: usize| -> bool {
        // The letter before, past marks, that joins forwards.
        chars[..index]
            .iter()
            .rev()
            .find(|c| !is_transparent(**c))
            .and_then(|c| forms(*c))
            .is_some_and(|(_, joining)| joining == Joining::Dual)
    };
    let joins_after = |index: usize| -> bool {
        chars[index + 1..]
            .iter()
            .find(|c| !is_transparent(**c))
            .and_then(|c| forms(*c))
            .is_some_and(|(_, joining)| joining != Joining::None)
    };
    let mut shaped = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let Some((isolated, joining)) = forms(c) else {
            shaped.push(c);
            index += 1;
            continue;
        };
        let before = joins_before(index);
        // A lam and the alef after it make one glyph.
        if u32::from(c) == 0x0644 {
            if let Some((alone, joined)) = chars.get(index + 1).and_then(|next| lam_alef(*next)) {
                shaped.push(if before { joined } else { alone });
                index += 2;
                continue;
            }
        }
        let offset = match (joining, before, joins_after(index)) {
            (Joining::None, ..) | (_, false, false) => 0,
            (_, true, false) => 1,
            (Joining::Dual, false, true) => 2,
            (Joining::Dual, true, true) => 3,
            (Joining::Right, false, true) => 0,
            (Joining::Right, true, true) => 1,
        };
        shaped.push(char::from_u32(isolated + offset).unwrap_or(c));
        index += 1;
    }
    shaped
}

#[cfg(test)]
#[path = "arabic_shaping/tests.rs"]
mod tests;

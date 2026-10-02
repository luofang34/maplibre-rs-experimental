//! Right-to-left text in the order it reads on a line.
use std::ops::Range;

use unicode_bidi::{bidi_class, BidiClass, BidiInfo};

/// Whether `c` reads from right to left.
fn is_rtl(c: char) -> bool {
    matches!(
        bidi_class(c),
        BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI
    )
}

/// Whether `c` only steers the bidirectional algorithm: marks, embeddings, overrides and
/// isolates, which GL JS's bidirectional pass leaves out of the text it draws.
pub(super) fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// The lines with each also ended after any paragraph separator in it: GL JS's bidirectional
/// pass takes the separators as paragraph ends on top of the breaks that fit the width.
pub(super) fn split_paragraphs(chars: &[char], lines: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut split = Vec::with_capacity(lines.len());
    for line in lines {
        let mut start = line.start;
        for index in line.clone() {
            if index + 1 < line.end && bidi_class(chars[index]) == BidiClass::B {
                split.push(start..index + 1);
                start = index + 1;
            }
        }
        split.push(start..line.end);
    }
    split
}

/// The indices of the characters of `line`, in the order they are drawn from left to right.
fn visual_order(chars: &[char], line: Range<usize>) -> Vec<usize> {
    let logical: Vec<usize> = line.clone().collect();
    if !chars[line.clone()].iter().any(|c| is_rtl(*c)) {
        return logical;
    }
    let text: String = chars[line.clone()].iter().collect();
    // The byte at which each character starts.
    let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    let char_at = |byte: usize| starts.partition_point(|start| *start < byte);
    let info = BidiInfo::new(&text, None);
    let mut order = Vec::with_capacity(logical.len());
    for paragraph in &info.paragraphs {
        let (levels, runs) = info.visual_runs(paragraph, paragraph.range.clone());
        for run in runs {
            let mut indices: Vec<usize> = (char_at(run.start)..char_at(run.end)).collect();
            if levels[run.start].is_rtl() {
                indices.reverse();
            }
            order.extend(indices);
        }
    }
    if order.len() != logical.len() {
        return logical;
    }
    order.into_iter().map(|index| line.start + index).collect()
}

/// Puts the characters of each line, and what goes with each, in the order they read on the line.
pub(super) fn reorder_lines<T: Clone>(
    chars: &mut [char],
    styles: &mut [T],
    lines: &[Range<usize>],
) {
    for line in lines {
        let order = visual_order(chars, line.clone());
        let (reordered_chars, reordered_styles): (Vec<char>, Vec<T>) = order
            .iter()
            .map(|index| (chars[*index], styles[*index].clone()))
            .unzip();
        chars[line.clone()].copy_from_slice(&reordered_chars);
        styles[line.clone()].clone_from_slice(&reordered_styles);
    }
}

#[cfg(test)]
#[path = "bidirectional/tests.rs"]
mod tests;

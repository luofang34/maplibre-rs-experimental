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

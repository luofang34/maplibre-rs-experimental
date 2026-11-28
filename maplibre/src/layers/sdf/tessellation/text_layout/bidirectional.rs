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
            if index + 1 < line.end
                && chars
                    .get(index)
                    .is_some_and(|ch| bidi_class(*ch) == BidiClass::B)
            {
                split.push(start..index + 1);
                start = index + 1;
            }
        }
        split.push(start..line.end);
    }
    split
}

fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{05BF}'
            | '\u{05C1}'..='\u{05C2}'
            | '\u{05C4}'..='\u{05C5}'
            | '\u{05C7}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{06D6}'..='\u{06DC}'
            | '\u{06DF}'..='\u{06E4}'
            | '\u{06E7}'..='\u{06E8}'
            | '\u{06EA}'..='\u{06ED}'
            | '\u{0900}'..='\u{0903}'
            | '\u{093A}'..='\u{094F}'
            | '\u{0951}'..='\u{0957}'
            | '\u{0962}'..='\u{0963}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// UAX #9 Rule L4 bracket/delimiter mirroring for odd (RTL) bidi levels.
fn mirror_bidi_char(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        '⟨' => '⟩',
        '⟩' => '⟨',
        '⟪' => '⟫',
        '⟫' => '⟪',
        '（' => '）',
        '）' => '（',
        '［' => '］',
        '］' => '［',
        '｛' => '｝',
        '｝' => '｛',
        '＜' => '＞',
        '＞' => '＜',
        '「' => '」',
        '」' => '「',
        '『' => '』',
        '』' => '『',
        '【' => '】',
        '】' => '【',
        '〔' => '〕',
        '〕' => '〔',
        '〈' => '〉',
        '〉' => '〈',
        '《' => '》',
        '》' => '《',
        _ => c,
    }
}

fn reverse_clusters_in_run(chars: &[char], indices: &[usize]) -> Vec<usize> {
    let mut clusters: Vec<Range<usize>> = Vec::with_capacity(indices.len());
    let mut start = 0;
    while start < indices.len() {
        let mut end = start + 1;
        while indices
            .get(end)
            .and_then(|&idx| chars.get(idx))
            .is_some_and(|ch| is_combining_mark(*ch))
        {
            end += 1;
        }
        clusters.push(start..end);
        start = end;
    }
    let mut reversed = Vec::with_capacity(indices.len());
    for cluster in clusters.into_iter().rev() {
        if let Some(slice) = indices.get(cluster) {
            reversed.extend_from_slice(slice);
        }
    }
    reversed
}

fn visual_order_for_line(
    chars: &[char],
    para_start: usize,
    line: Range<usize>,
    info: &BidiInfo<'_>,
    starts: &[usize],
) -> Vec<(usize, bool)> {
    let logical: Vec<(usize, bool)> = line.clone().map(|idx| (idx, false)).collect();
    let Some(para) = info.paragraphs.first() else {
        return logical;
    };
    if line.is_empty() {
        return logical;
    }
    let char_at = |byte: usize| starts.partition_point(|start| *start < byte);
    let Some(&byte_start) = line
        .start
        .checked_sub(para_start)
        .and_then(|i| starts.get(i))
    else {
        return logical;
    };
    let Some(&byte_end) = line.end.checked_sub(para_start).and_then(|i| starts.get(i)) else {
        return logical;
    };
    let clamped = byte_start.max(para.range.start)..byte_end.min(para.range.end);
    if clamped.is_empty() {
        return logical;
    }
    let (levels, runs) = info.visual_runs(para, clamped);
    let mut order = Vec::with_capacity(line.len());
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(|lvl| lvl.is_rtl());
        let abs_indices: Vec<usize> = (char_at(run.start)..char_at(run.end))
            .map(|local_idx| para_start + local_idx)
            .collect();
        if rtl {
            for idx in reverse_clusters_in_run(chars, &abs_indices) {
                order.push((idx, true));
            }
        } else {
            for idx in abs_indices {
                order.push((idx, false));
            }
        }
    }
    if order.len() != line.len() {
        return logical;
    }
    order
}

/// Puts the characters of each line, and what goes with each, in the order they read on the line,
/// preserving paragraph-level base direction across wrapped lines and UAX #9 L4 bracket mirroring.
pub(super) fn reorder_lines<T: Clone>(
    chars: &mut [char],
    styles: &mut [T],
    lines: &[Range<usize>],
) {
    let mut i = 0;
    while let Some(first_line) = lines.get(i) {
        let para_start = first_line.start;
        let mut group_end = i + 1;
        while let (Some(prev_line), Some(next_line)) =
            (lines.get(group_end - 1), lines.get(group_end))
        {
            let prev_end = prev_line.end;
            let next_start = next_line.start;
            let gap_has_break = next_start < prev_end
                || chars
                    .get(prev_line.clone())
                    .and_then(|s| s.last())
                    .is_some_and(|c| *c == '\n' || bidi_class(*c) == BidiClass::B)
                || chars
                    .get(prev_end..next_start)
                    .unwrap_or(&[])
                    .iter()
                    .any(|c| *c == '\n' || bidi_class(*c) == BidiClass::B);
            if gap_has_break {
                break;
            }
            group_end += 1;
        }
        let para_end = lines.get(group_end - 1).map_or(para_start, |l| l.end);
        let slice = chars.get(para_start..para_end).unwrap_or(&[]);
        let has_rtl_or_ctrl = slice.iter().any(|c| is_rtl(*c) || is_bidi_control(*c));
        if has_rtl_or_ctrl {
            let text: String = slice.iter().collect();
            let mut starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
            starts.push(text.len());
            let info = BidiInfo::new(&text, None);
            for line in lines.get(i..group_end).unwrap_or(&[]) {
                let order = visual_order_for_line(chars, para_start, line.clone(), &info, &starts);
                let (reordered_chars, reordered_styles): (Vec<char>, Vec<T>) = order
                    .iter()
                    .filter_map(|&(index, rtl)| {
                        let &raw = chars.get(index)?;
                        let ch = if rtl { mirror_bidi_char(raw) } else { raw };
                        let style = styles.get(index)?.clone();
                        Some((ch, style))
                    })
                    .unzip();
                if let (Some(c_dst), Some(s_dst)) =
                    (chars.get_mut(line.clone()), styles.get_mut(line.clone()))
                {
                    if c_dst.len() == reordered_chars.len() && s_dst.len() == reordered_styles.len()
                    {
                        c_dst.copy_from_slice(&reordered_chars);
                        s_dst.clone_from_slice(&reordered_styles);
                    }
                }
            }
        }
        i = group_end;
    }
}

/// Reorders `lines` using bidirectional controls and strips `is_bidi_control` characters afterwards,
/// remapping each line range to the stripped slices.
pub(super) fn reorder_and_strip_controls<T: Clone>(
    chars: &mut Vec<char>,
    styles: &mut Vec<T>,
    lines: Vec<Range<usize>>,
) -> Vec<Range<usize>> {
    reorder_lines(chars, styles, &lines);
    let mut new_chars = Vec::with_capacity(chars.len());
    let mut new_styles = Vec::with_capacity(styles.len());
    let mut new_lines = Vec::with_capacity(lines.len());
    for line in lines {
        let start = new_chars.len();
        for (&ch, style) in chars
            .get(line.clone())
            .into_iter()
            .flatten()
            .zip(styles.get(line).into_iter().flatten())
        {
            if !is_bidi_control(ch) {
                new_chars.push(ch);
                new_styles.push(style.clone());
            }
        }
        let end = new_chars.len();
        new_lines.push(start..end);
    }
    *chars = new_chars;
    *styles = new_styles;
    new_lines
}

#[cfg(test)]
#[path = "bidirectional/tests.rs"]
mod tests;

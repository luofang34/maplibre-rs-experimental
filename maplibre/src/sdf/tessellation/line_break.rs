//! Where a label breaks into lines, by the same badness GL JS minimizes.
use std::ops::Range;

/// A place the text may break, with the least raggedness of any way to reach it.
struct Break {
    index: usize,
    x: f32,
    prior: Option<usize>,
    badness: f32,
}

fn is_whitespace(c: char) -> bool {
    c.is_whitespace()
}

/// Characters after which a line may break.
fn breakable(c: char) -> bool {
    matches!(
        c,
        // An image in text may be followed by a break.
        crate::style::expression::FORMAT_IMAGE
            | '\n'
            | ' '
            | '&'
            | ')'
            | '+'
            | '-'
            | '/'
            | '\u{ad}'
            | '\u{b7}'
            | '\u{200b}'
            | '\u{2010}'
            | '\u{2013}'
            | '\u{2027}'
    )
}

/// Ideographic characters, which may break between any two of them.
fn allows_ideographic_breaking(c: char) -> bool {
    matches!(
        u32::from(c),
        0x2E80..=0x2FDF | 0x3000..=0x30FF | 0x3100..=0x9FFF | 0xA000..=0xA4CF | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F
    )
}

fn break_penalty(c: char, next: char, penalizable_ideographic_break: bool) -> f32 {
    let mut penalty = 0.0;
    if c == '\n' {
        penalty -= 10_000.0;
    }
    if penalizable_ideographic_break {
        penalty += 150.0;
    }
    if c == '(' || c == '\u{ff08}' {
        penalty += 50.0;
    }
    if next == ')' || next == '\u{ff09}' {
        penalty += 50.0;
    }
    penalty
}

fn badness(line_width: f32, target: f32, penalty: f32, last: bool) -> f32 {
    let raggedness = (line_width - target).powi(2);
    if last {
        // Final lines shorter than the average are favoured over longer ones.
        if line_width < target {
            raggedness / 2.0
        } else {
            raggedness * 2.0
        }
    } else {
        raggedness + penalty.abs() * penalty
    }
}

fn evaluate_break(
    (index, x): (usize, f32),
    (target, penalty, last): (f32, f32, bool),
    potential: &[Break],
) -> Break {
    let mut prior = None;
    let mut best = badness(x, target, penalty, last);
    for (at, candidate) in potential.iter().enumerate() {
        let total = badness(x - candidate.x, target, penalty, last) + candidate.badness;
        if total <= best {
            prior = Some(at);
            best = total;
        }
    }
    Break {
        index,
        x,
        prior,
        badness: best,
    }
}

/// Breaks the text into lines the way GL JS does: at the places that leave the lines closest
/// to the same width, with the width a line may run to only steering how many lines there are.
/// `advance` gives the width of each character, spacing included; the lines come back as
/// ranges of `chars` without the white space at their ends.
pub(super) fn wrap(
    chars: &[char],
    max_width: f32,
    advance: &dyn Fn(usize) -> f32,
) -> Vec<Range<usize>> {
    let total: f32 = (0..chars.len()).map(advance).sum();
    let target = total / (total / max_width).ceil().max(1.0);
    let has_zero_width_space = chars.contains(&'\u{200b}');
    let mut potential: Vec<Break> = Vec::new();
    let mut x = 0.0;
    for (index, c) in chars.iter().copied().enumerate() {
        if !is_whitespace(c) {
            x += advance(index);
        }
        let Some(next) = chars.get(index + 1).copied() else {
            continue;
        };
        let ideographic = allows_ideographic_breaking(c);
        if breakable(c) || ideographic || (index + 2 < chars.len() && next == '(') {
            let penalty = break_penalty(c, next, ideographic && has_zero_width_space);
            let found = evaluate_break((index + 1, x), (target, penalty, false), &potential);
            potential.push(found);
        }
    }
    let last = evaluate_break((chars.len(), x), (target, 0.0, true), &potential);
    let mut ends = vec![last.index];
    let mut prior = last.prior;
    while let Some(at) = prior {
        ends.push(potential[at].index);
        prior = potential[at].prior;
    }
    ends.reverse();
    let mut lines = Vec::new();
    let mut start = 0;
    for end in ends {
        lines.push(trimmed(chars, start..end));
        start = end;
    }
    if start < chars.len() {
        lines.push(trimmed(chars, start..chars.len()));
    }
    lines
}

fn trimmed(chars: &[char], Range { mut start, mut end }: Range<usize>) -> Range<usize> {
    while start < end && chars[start].is_whitespace() {
        start += 1;
    }
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    start..end
}

#[cfg(test)]
mod tests;

//! Joins the lines of a layer that end where another begins and carry the same text, as GL JS
//! `mergeLines` does before it places labels along them, so a road cut into pieces is labelled
//! as one road.

use std::collections::HashMap;

use crate::style::expression::FeatureProperties;

/// One line to be labelled, with what its label is made from.
pub(super) struct PendingLine {
    /// The text of the label; a line without text is never merged.
    pub text: Option<String>,
    pub line: Vec<[f64; 2]>,
    pub source: super::layout::SourceFeature,
    pub properties: FeatureProperties,
}

struct Chunk {
    points: Vec<[f64; 2]>,
    next: Option<usize>,
}

struct Merged {
    feature: usize,
    first: usize,
    last: usize,
    alive: bool,
}

fn key(text: &str, chunks: &[Chunk], merged: &Merged, on_right: bool) -> String {
    let points = &chunks[if on_right { merged.last } else { merged.first }].points;
    let point = if on_right {
        points[points.len() - 1]
    } else {
        points[0]
    };
    format!("{text}:{}:{}", point[0], point[1])
}

struct Lines {
    chunks: Vec<Chunk>,
    merged: Vec<Merged>,
    left: HashMap<String, usize>,
    right: HashMap<String, usize>,
}

impl Lines {
    /// Appends `line` to the merged line that ends where it starts.
    fn merge_from_right(&mut self, keys: (&str, &str), line: (usize, usize)) -> usize {
        let (left_key, right_key) = keys;
        let i = self.right.remove(left_key).unwrap_or_default();
        self.right.insert(right_key.to_owned(), i);
        let last = self.merged[i].last;
        self.chunks[last].next = Some(line.0);
        self.merged[i].last = line.1;
        i
    }

    /// Prepends `line` to the merged line that starts where it ends.
    fn merge_from_left(&mut self, keys: (&str, &str), line: (usize, usize)) -> usize {
        let (left_key, right_key) = keys;
        let i = self.left.remove(right_key).unwrap_or_default();
        self.left.insert(left_key.to_owned(), i);
        let first = self.merged[i].first;
        self.chunks[line.1].next = Some(first);
        self.merged[i].first = line.0;
        i
    }

    fn joined(&self, merged: &Merged) -> Vec<[f64; 2]> {
        let mut points = self.chunks[merged.first].points.clone();
        let mut next = self.chunks[merged.first].next;
        while let Some(index) = next {
            points.extend_from_slice(&self.chunks[index].points[1..]);
            next = self.chunks[index].next;
        }
        points
    }
}

/// The lines with those that join end to start and share their text made into one.
pub(super) fn merge_lines(features: Vec<PendingLine>) -> Vec<PendingLine> {
    let mut lines = Lines {
        chunks: Vec::new(),
        merged: Vec::new(),
        left: HashMap::new(),
        right: HashMap::new(),
    };
    for (index, feature) in features.iter().enumerate() {
        lines.chunks.push(Chunk {
            points: feature.line.clone(),
            next: None,
        });
        let chunk = lines.chunks.len() - 1;
        let line = Merged {
            feature: index,
            first: chunk,
            last: chunk,
            alive: true,
        };
        let text = match &feature.text {
            Some(text) if feature.line.len() > 1 && !text.is_empty() => text.clone(),
            _ => {
                lines.merged.push(line);
                continue;
            }
        };
        let left_key = key(&text, &lines.chunks, &line, false);
        let right_key = key(&text, &lines.chunks, &line, true);
        let keys = (left_key.as_str(), right_key.as_str());
        let span = (chunk, chunk);
        let touches_both = lines.right.get(&left_key).zip(lines.left.get(&right_key));
        if touches_both.is_some_and(|(right, left)| right != left) {
            // Lines with the same text meet both ends of this one: join all three.
            let j = lines.merge_from_left(keys, span);
            let joined = (lines.merged[j].first, lines.merged[j].last);
            let i = lines.merge_from_right(keys, joined);
            lines.left.remove(&left_key);
            lines.right.remove(&right_key);
            let end = key(&text, &lines.chunks, &lines.merged[i], true);
            lines.right.insert(end, i);
            lines.merged[j].alive = false;
        } else if lines.right.contains_key(&left_key) {
            lines.merge_from_right(keys, span);
        } else if lines.left.contains_key(&right_key) {
            lines.merge_from_left(keys, span);
        } else {
            lines.merged.push(line);
            let i = lines.merged.len() - 1;
            lines.left.insert(left_key, i);
            lines.right.insert(right_key, i);
        }
    }
    let mut slots: Vec<Option<PendingLine>> = features.into_iter().map(Some).collect();
    let mut result = Vec::new();
    for merged in &lines.merged {
        if !merged.alive {
            continue;
        }
        let Some(mut feature) = slots[merged.feature].take() else {
            continue;
        };
        feature.line = lines.joined(merged);
        result.push(feature);
    }
    result
}

#[cfg(test)]
mod tests;

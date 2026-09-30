#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn road(text: Option<&str>, points: &[[f64; 2]]) -> PendingLine {
    PendingLine {
        text: text.map(str::to_owned),
        line: points.to_vec(),
        id: None,
        properties: FeatureProperties::new(),
    }
}

#[test]
fn pieces_with_the_same_text_that_meet_end_to_start_become_one_line() {
    let merged = merge_lines(vec![
        road(Some("Main"), &[[0.0, 0.0], [10.0, 0.0]]),
        road(Some("Main"), &[[10.0, 0.0], [20.0, 5.0]]),
    ]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].line, [[0.0, 0.0], [10.0, 0.0], [20.0, 5.0]]);
}

#[test]
fn a_piece_that_meets_both_neighbours_joins_all_three() {
    let merged = merge_lines(vec![
        road(Some("Main"), &[[0.0, 0.0], [10.0, 0.0]]),
        road(Some("Main"), &[[20.0, 0.0], [30.0, 0.0]]),
        road(Some("Main"), &[[10.0, 0.0], [20.0, 0.0]]),
    ]);
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].line,
        [[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]]
    );
}

#[test]
fn lines_with_other_text_or_no_text_stay_apart() {
    let merged = merge_lines(vec![
        road(Some("Main"), &[[0.0, 0.0], [10.0, 0.0]]),
        road(Some("Side"), &[[10.0, 0.0], [20.0, 0.0]]),
        road(None, &[[20.0, 0.0], [30.0, 0.0]]),
    ]);
    assert_eq!(merged.len(), 3);
}

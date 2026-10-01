#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

#[test]
fn kana_and_ideographs_allow_vertical_writing_but_latin_does_not() {
    assert!(allows_vertical_writing(&chars("マップ")));
    assert!(allows_vertical_writing(&chars("一二三")));
    assert!(allows_vertical_writing(&chars("abc ボ")));
    assert!(!allows_vertical_writing(&chars("Latin 123")));
    assert!(!allows_vertical_writing(&chars("نشاط")));
}

#[test]
fn spaces_and_arabic_lie_on_their_side() {
    assert!(is_upright('a'));
    assert!(is_upright('ボ'));
    assert!(!is_upright(' '));
    assert!(!is_upright('ن'));
}

#[test]
fn a_turned_box_swaps_its_axes_about_the_anchor() {
    assert_eq!(
        turned_box([-10.0, -4.0, 30.0, 6.0]),
        [-6.0, -10.0, 4.0, 30.0]
    );
}

#[test]
fn an_anchor_shifts_the_turned_box_to_the_side_it_names() {
    let box_size = [20.0, 40.0];
    assert_eq!(anchor_shift("center", box_size, [0.0, 0.0]), [0.0, 0.0]);
    assert_eq!(anchor_shift("left", box_size, [3.0, 0.0]), [13.0, 0.0]);
    assert_eq!(anchor_shift("bottom", box_size, [0.0, -2.0]), [0.0, -22.0]);
}

#[test]
fn a_vertical_line_counts_an_em_for_an_upright_glyph() {
    use crate::sdf::assets::AtlasEntry;
    let mut glyphs = std::collections::HashMap::new();
    for c in ['ボ', 'ن'] {
        glyphs.insert(
            c as u32,
            AtlasEntry {
                metrics: [0.0, 0.0, 10.0, 1.0],
                ..AtlasEntry::default()
            },
        );
    }
    let style = CharStyle {
        scale: 1.0,
        color: None,
        glyphs: &glyphs,
        image: None,
    };
    let text = chars("ボن");
    let styles = vec![style.clone(), style];
    // One em for the upright glyph, the glyph's own advance for the one on its side.
    assert_eq!(line_length(&text, &styles, 0..2, 0.0), 24.0 + 10.0);
}

//! Text and icon overlap, dependency, and collision insertion rules.
use super::super::collision_grid::CollisionGrid;
use crate::style::{
    expression::FeatureProperties,
    layer::{StyleProperty, SymbolPaint},
};

/// How a symbol treats overlap, from `text-overlap` and `icon-overlap`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlap {
    /// Collides with every placed symbol.
    Never,
    /// Never collides.
    Always,
    /// Collides only with symbols that are not cooperative.
    Cooperative,
}

pub(super) struct PlacementRules {
    overlap: [Overlap; 2],
    ignore: [bool; 2],
    optional: [bool; 2],
}

impl PlacementRules {
    pub(super) fn new(paint: &SymbolPaint, properties: &FeatureProperties, zoom: f64) -> Self {
        let read = |suffix: &str| {
            ["text", "icon"].map(|prefix| {
                paint
                    .properties
                    .get(&format!("{prefix}-{suffix}"))
                    .and_then(|value| {
                        StyleProperty::<bool>::parse(value).evaluate_for(properties, zoom)
                    })
                    .unwrap_or(false)
            })
        };
        let allow = read("allow-overlap");
        // The newer property replaces the boolean when a style sets both.
        let overlap = [0, 1].map(|i| {
            let name = format!("{}-overlap", ["text", "icon"][i]);
            match paint.properties.get(&name).and_then(|value| value.as_str()) {
                Some("always") => Overlap::Always,
                Some("cooperative") => Overlap::Cooperative,
                Some(_) => Overlap::Never,
                None if allow[i] => Overlap::Always,
                None => Overlap::Never,
            }
        });
        Self {
            overlap,
            ignore: read("ignore-placement"),
            optional: read("optional"),
        }
    }

    #[cfg(test)]
    pub(super) fn place(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        grid: &mut CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        self.place_along_line(rectangles, &[], grid, viewport)
    }

    /// Places a label whose text collides through `glyph_boxes`, one box per glyph along its
    /// line, instead of through the single rectangle around all of it. The rectangle still
    /// decides whether the label is on screen.
    pub(super) fn place_along_line(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
        grid: &mut CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        let visible = self.visible(rectangles, glyph_boxes, grid, viewport);
        for (i, shown) in visible.into_iter().enumerate() {
            if shown && !self.ignore[i] {
                for part in Self::collision_boxes(i, rectangles, glyph_boxes) {
                    grid.insert_as(part, self.overlap[i] == Overlap::Cooperative);
                }
            }
        }
        visible
    }

    fn collision_boxes(
        i: usize,
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
    ) -> Vec<[f64; 4]> {
        if i == 0 && !glyph_boxes.is_empty() {
            glyph_boxes.to_vec()
        } else {
            rectangles[i].into_iter().collect()
        }
    }

    /// Which parts of a label would be shown, without taking their place in the grid.
    pub(super) fn visible(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
        grid: &CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        self.visible_as(self.overlap, rectangles, glyph_boxes, grid, viewport)
    }

    /// Like `visible`, with the text treated as one that may not overlap: the first attempt of
    /// a label with variable anchors, before it accepts overlap at its first anchor.
    pub(super) fn visible_without_text_overlap(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
        grid: &CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        let overlap = [Overlap::Never, self.overlap[1]];
        self.visible_as(overlap, rectangles, glyph_boxes, grid, viewport)
    }

    fn visible_as(
        &self,
        overlap: [Overlap; 2],
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
        grid: &CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        let accepted = [0, 1].map(|i| {
            rectangles[i].is_some_and(|rect| {
                rect.iter().all(|v| v.is_finite())
                    && rect[2] >= 0.0
                    && rect[3] >= 0.0
                    && rect[0] <= viewport[0]
                    && rect[1] <= viewport[1]
                    && Self::collision_boxes(i, rectangles, glyph_boxes)
                        .iter()
                        .all(|part| match overlap[i] {
                            Overlap::Always => true,
                            Overlap::Never => !grid.overlaps(*part),
                            Overlap::Cooperative => !grid.overlaps_non_cooperative(*part),
                        })
            })
        });
        [0, 1].map(|i| {
            accepted[i] && (rectangles[1 - i].is_none() || accepted[1 - i] || self.optional[1 - i])
        })
    }
}

#[cfg(test)]
mod tests;

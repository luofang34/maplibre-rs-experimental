//! Text and icon overlap, dependency, and collision insertion rules.
use crate::{
    sdf::collision_grid::CollisionGrid,
    style::{expression::FeatureProperties, layer::SymbolPaint},
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

#[derive(Clone, Copy)]
pub(in crate::layers::sdf::collision_system) struct PlacementRules {
    overlap: [Overlap; 2],
    ignore: [bool; 2],
    optional: [bool; 2],
}

impl PlacementRules {
    pub(in crate::layers::sdf::collision_system) fn constant_for_layer(
        paint: &SymbolPaint,
        zoom: f64,
    ) -> Option<Self> {
        let is_const = |name: &str| {
            paint
                .compiled_property::<bool>(name)
                .is_none_or(|prop| prop.is_feature_constant())
        };
        if ![
            "text-allow-overlap",
            "icon-allow-overlap",
            "text-ignore-placement",
            "icon-ignore-placement",
            "text-optional",
            "icon-optional",
        ]
        .into_iter()
        .all(is_const)
        {
            return None;
        }
        Some(Self::new(paint, &FeatureProperties::default(), zoom))
    }

    pub(in crate::layers::sdf::collision_system) fn new(
        paint: &SymbolPaint,
        properties: &FeatureProperties,
        zoom: f64,
    ) -> Self {
        let read = |names: [&str; 2]| {
            names.map(|name| {
                paint
                    .compiled_property::<bool>(name)
                    .and_then(|prop| {
                        if prop.is_feature_constant() {
                            prop.evaluate_at_zoom(zoom)
                        } else {
                            prop.evaluate_for(properties, zoom)
                        }
                    })
                    .unwrap_or(false)
            })
        };
        let allow = read(["text-allow-overlap", "icon-allow-overlap"]);
        let overlap_for = |name: &str, fallback: bool| {
            // Layout follows the integer zoom a tile is laid out at, not the view's fraction.
            match paint.layout_enum(name, zoom.floor()).as_deref() {
                Some("always") => Overlap::Always,
                Some("cooperative") => Overlap::Cooperative,
                Some(_) => Overlap::Never,
                None if fallback => Overlap::Always,
                None => Overlap::Never,
            }
        };
        // The newer property replaces the boolean when a style sets both.
        let overlap = [
            overlap_for("text-overlap", allow[0]),
            overlap_for("icon-overlap", allow[1]),
        ];
        Self {
            overlap,
            ignore: read(["text-ignore-placement", "icon-ignore-placement"]),
            optional: read(["text-optional", "icon-optional"]),
        }
    }

    #[cfg(test)]
    pub(in crate::layers::sdf::collision_system) fn place(
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
    pub(in crate::layers::sdf::collision_system) fn place_along_line(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        glyph_boxes: &[[f64; 4]],
        grid: &mut CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        let visible = self.visible(rectangles, glyph_boxes, grid, viewport);
        if visible[0] && !self.ignore[0] {
            for part in Self::text_collision_boxes(rectangles[0], glyph_boxes) {
                grid.insert_as(part, self.overlap[0] == Overlap::Cooperative);
            }
        }
        if visible[1] && !self.ignore[1] {
            if let Some(part) = rectangles[1] {
                grid.insert_as(part, self.overlap[1] == Overlap::Cooperative);
            }
        }
        visible
    }

    fn text_collision_boxes(rect: Option<[f64; 4]>, glyph_boxes: &[[f64; 4]]) -> Vec<[f64; 4]> {
        if !glyph_boxes.is_empty() {
            glyph_boxes.to_vec()
        } else {
            rect.into_iter().collect()
        }
    }

    /// Which parts of a label would be shown, without taking their place in the grid.
    pub(in crate::layers::sdf::collision_system) fn visible(
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
    pub(in crate::layers::sdf::collision_system) fn visible_without_text_overlap(
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
        let accepts = |rect: Option<[f64; 4]>, boxes: Vec<[f64; 4]>, mode: Overlap| {
            rect.is_some_and(|rect| {
                rect.iter().all(|v| v.is_finite())
                    && rect[2] >= -VIEWPORT_PADDING
                    && rect[3] >= -VIEWPORT_PADDING
                    && rect[0] < viewport[0] + VIEWPORT_PADDING
                    && rect[1] < viewport[1] + VIEWPORT_PADDING
                    && boxes.iter().all(|part| match mode {
                        Overlap::Always => true,
                        Overlap::Never => !grid.overlaps(*part),
                        Overlap::Cooperative => !grid.overlaps_non_cooperative(*part),
                    })
            })
        };
        let text_ok = accepts(
            rectangles[0],
            Self::text_collision_boxes(rectangles[0], glyph_boxes),
            overlap[0],
        );
        let icon_ok = accepts(
            rectangles[1],
            rectangles[1].into_iter().collect(),
            overlap[1],
        );
        [
            text_ok && (rectangles[1].is_none() || icon_ok || self.optional[1]),
            icon_ok && (rectangles[0].is_none() || text_ok || self.optional[0]),
        ]
    }
}

/// How far past the viewport edge a box still takes part in placement, as in GL JS, so a label
/// whose icon is just off screen keeps its text and labels slide in without popping.
const VIEWPORT_PADDING: f64 = 100.0;

#[cfg(test)]
mod tests;

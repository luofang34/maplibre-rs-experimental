//! Text and icon overlap, dependency, and collision insertion rules.
use super::super::collision_grid::CollisionGrid;
use crate::style::{
    expression::FeatureProperties,
    layer::{StyleProperty, SymbolPaint},
};

pub(super) struct PlacementRules {
    allow: [bool; 2],
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
        Self {
            allow: read("allow-overlap"),
            ignore: read("ignore-placement"),
            optional: read("optional"),
        }
    }

    pub(super) fn place(
        &self,
        rectangles: [Option<[f64; 4]>; 2],
        grid: &mut CollisionGrid,
        viewport: [f64; 2],
    ) -> [bool; 2] {
        let accepted = [0, 1].map(|i| {
            rectangles[i].is_some_and(|rect| {
                rect.iter().all(|v| v.is_finite())
                    && rect[2] >= 0.0
                    && rect[3] >= 0.0
                    && rect[0] <= viewport[0]
                    && rect[1] <= viewport[1]
                    && (self.allow[i] || !grid.overlaps(rect))
            })
        });
        let visible = [0, 1].map(|i| {
            accepted[i] && (rectangles[1 - i].is_none() || accepted[1 - i] || self.optional[1 - i])
        });
        for i in 0..2 {
            if visible[i] && !self.ignore[i] {
                if let Some(rect) = rectangles[i] {
                    grid.insert(rect);
                }
            }
        }
        visible
    }
}

#[cfg(test)]
mod tests;

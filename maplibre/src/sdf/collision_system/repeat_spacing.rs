//! Keeps the repeats of a label along its lines apart on screen, across tiles.
//!
//! `symbol-spacing` spaces a label's repeats along a line in the tile's own pixels, so a tilted
//! or narrow view that shows a tile magnified near its far edge, or many tiles of one road,
//! packs the same road shield every hundred pixels or so on screen. This rule measures the
//! repeats where they are drawn instead.
use std::{collections::HashMap, sync::Arc};

use crate::sdf::{Feature, SymbolLayerData};

/// The least distance on screen, in logical pixels, between two repeats of one label in one
/// style layer.
///
/// It is the default `symbol-spacing`: the gap a style gets between repeats when a tile is
/// seen flat at its own zoom, which this keeps at any zoom, tilt or field of view. Collision
/// boxes are measured in logical pixels, the units labels are sized in, so the gap scales
/// with the labels on a display of any pixel ratio.
pub(super) const MIN_REPEAT_DISTANCE: f64 = 250.0;

type Positions = HashMap<String, HashMap<Arc<str>, Vec<[f64; 2]>>>;

/// Where the repeats of each label were placed on screen, by style layer and repeat key, in
/// this placement and the one before.
#[derive(Default)]
pub(super) struct RepeatSpacing {
    placed: Positions,
    previous: Positions,
}

/// A label that repeats along lines, and the centre of its box on screen.
pub(super) struct Repeat<'a> {
    layer: &'a str,
    key: &'a Arc<str>,
    at: [f64; 2],
}

impl<'a> Repeat<'a> {
    /// The repeat a label would place with `rectangles`, its text and icon boxes on screen;
    /// `None` for a label that does not repeat or whose box is not centred in `viewport`.
    ///
    /// A label just past the edge of the view is placed so that it can slide in, but nobody
    /// sees it there, so it neither keeps a repeat in view from its place nor is kept from its
    /// own.
    pub(super) fn of(
        layer: &'a SymbolLayerData,
        feature: &'a Feature,
        rectangles: &[Option<[f64; 4]>; 2],
        viewport: [f64; 2],
    ) -> Option<Self> {
        let key = feature.repeat_key.as_ref()?;
        let [left, top, right, bottom] = rectangles[0].or(rectangles[1])?;
        let at = [(left + right) * 0.5, (top + bottom) * 0.5];
        let in_view = (0.0..=viewport[0]).contains(&at[0]) && (0.0..=viewport[1]).contains(&at[1]);
        in_view.then_some(Self {
            layer: &layer.style_layer_id,
            key,
            at,
        })
    }
}

impl RepeatSpacing {
    /// Starts a placement; the one that ends becomes the previous.
    pub(super) fn begin(&mut self) {
        self.previous = std::mem::take(&mut self.placed);
    }

    /// Whether `repeat` would sit closer than [`MIN_REPEAT_DISTANCE`] to a repeat of its label
    /// placed earlier in this placement or, unless it was shown itself, in the previous one.
    ///
    /// Placement goes in a fixed order, so the first of two close repeats keeps its place
    /// while the view holds still. A repeat that was not shown also yields to those shown
    /// before, wherever they come in the order, so a repeat that a newly loaded tile brings,
    /// or that the moving view brings near, does not take the place of one already shown.
    pub(super) fn too_close(&self, repeat: &Repeat<'_>, was_visible: bool) -> bool {
        let near = |positions: &Positions| {
            positions
                .get(repeat.layer)
                .and_then(|keys| keys.get(repeat.key))
                .is_some_and(|placed| {
                    placed.iter().any(|other| {
                        (other[0] - repeat.at[0]).hypot(other[1] - repeat.at[1])
                            < MIN_REPEAT_DISTANCE
                    })
                })
        };
        near(&self.placed) || (!was_visible && near(&self.previous))
    }

    /// Records that `repeat` took its place.
    pub(super) fn remember(&mut self, repeat: Repeat<'_>) {
        self.placed
            .entry(repeat.layer.to_owned())
            .or_default()
            .entry(repeat.key.clone())
            .or_default()
            .push(repeat.at);
    }
}

#[cfg(test)]
mod tests;

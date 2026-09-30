//! `line-cap`, `line-join` and `line-miter-limit`, the layout properties that shape a stroke.

use crate::style::layer::StyleLayer;

/// How the ends of a line are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineCap {
    /// Flush with the end point.
    #[default]
    Butt,
    /// A half circle around the end point.
    Round,
    /// A half square past the end point.
    Square,
}

/// How two segments of a line meet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineJoin {
    /// The outer edges are extended to a point, up to the miter limit.
    #[default]
    Miter,
    /// The corner is cut off flat.
    Bevel,
    /// The corner is an arc around the vertex.
    Round,
}

/// The stroke shape of a line layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineStroke {
    /// End style.
    pub cap: LineCap,
    /// Corner style.
    pub join: LineJoin,
    /// Longest miter, as a multiple of the half width, before a miter join becomes a bevel.
    pub miter_limit: f32,
}

impl Default for LineStroke {
    fn default() -> Self {
        Self {
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 2.0,
        }
    }
}

/// The layout properties [`LineStroke`] reads.
pub const STROKE_LAYOUT: [&str; 3] = ["line-cap", "line-join", "line-miter-limit"];

impl LineStroke {
    /// Reads the stroke properties from a layer's layout; anything else keeps its default.
    pub fn of_layer(layer: &StyleLayer) -> Self {
        let layout = &layer.unrecognized.layout;
        let mut stroke = Self::default();
        match layout.get("line-cap").and_then(|value| value.as_str()) {
            Some("round") => stroke.cap = LineCap::Round,
            Some("square") => stroke.cap = LineCap::Square,
            _ => {}
        }
        match layout.get("line-join").and_then(|value| value.as_str()) {
            Some("round") => stroke.join = LineJoin::Round,
            Some("bevel") => stroke.join = LineJoin::Bevel,
            _ => {}
        }
        if let Some(limit) = layout
            .get("line-miter-limit")
            .and_then(|value| value.as_f64())
        {
            stroke.miter_limit = limit as f32;
        }
        stroke
    }

    /// Whether a layout property is one of these and holds a value the stroke understands.
    pub fn accepts(name: &str, value: &serde_json::Value) -> bool {
        match name {
            "line-cap" => matches!(value.as_str(), Some("butt" | "round" | "square")),
            "line-join" => matches!(value.as_str(), Some("miter" | "bevel" | "round")),
            "line-miter-limit" => value.is_number(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;

//! Composite style-spec value types: `padding`, `numberArray`, `colorArray`,
//! `projectionDefinition`, and `variableAnchorOffsetCollection`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use super::{interpolate_number, number_to_json, Color, ColorSpace};

fn values_json(values: Vec<Json>) -> Json {
    json!({ "values": values })
}

fn premultiplied_color_object(color: &Color) -> Json {
    let [r, g, b, a] = color.premultiplied();
    json!({
        "r": number_to_json(r),
        "g": number_to_json(g),
        "b": number_to_json(b),
        "a": number_to_json(a),
    })
}

fn interpolate_offset_pair(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [
        interpolate_number(a[0], b[0], t),
        interpolate_number(a[1], b[1], t),
    ]
}

/// Directional `[top, right, bottom, left]` box padding (`padding`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Padding(pub [f64; 4]);

impl Padding {
    /// Componentwise linear interpolation between two padding values.
    pub fn interpolate(self, to: Self, t: f64) -> Self {
        Self([
            interpolate_number(self.0[0], to.0[0], t),
            interpolate_number(self.0[1], to.0[1], t),
            interpolate_number(self.0[2], to.0[2], t),
            interpolate_number(self.0[3], to.0[3], t),
        ])
    }

    /// Serializes to the GL JS `{ "values": [top, right, bottom, left] }` representation.
    pub fn to_json(self) -> Json {
        values_json(self.0.map(number_to_json).to_vec())
    }
}

/// Variable-length numeric array (`numberArray`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberArray(pub Vec<f64>);

impl NumberArray {
    /// Componentwise interpolation when both arrays have matching length.
    pub fn interpolate(&self, to: &Self, t: f64) -> Option<Self> {
        if self.0.len() != to.0.len() {
            return None;
        }
        Some(Self(
            self.0
                .iter()
                .zip(&to.0)
                .map(|(a, b)| interpolate_number(*a, *b, t))
                .collect(),
        ))
    }

    /// Serializes to `{ "values": [...] }`.
    pub fn to_json(&self) -> Json {
        values_json(self.0.iter().map(|v| number_to_json(*v)).collect())
    }
}

/// Variable-length colour array (`colorArray`).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorArray(pub Vec<Color>);

impl ColorArray {
    /// Componentwise colour interpolation in `space` when both arrays have matching length.
    pub fn interpolate(&self, to: &Self, t: f64, space: ColorSpace) -> Option<Self> {
        if self.0.len() != to.0.len() {
            return None;
        }
        Some(Self(
            self.0
                .iter()
                .zip(&to.0)
                .map(|(a, b)| Color::interpolate(*a, *b, t, space))
                .collect(),
        ))
    }

    /// Serializes to `{ "values": [{ "r", "g", "b", "a" }, ...] }` with premultiplied channels.
    pub fn to_json(&self) -> Json {
        values_json(self.0.iter().map(premultiplied_color_object).collect())
    }
}

/// Map projection definition (`projectionDefinition`): a named projection or transition blend.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectionDefinition {
    /// Named projection such as `"mercator"` or `"vertical-perspective"`.
    Named(String),
    /// Explicit `[from, to, transition]` array literal.
    Tuple {
        /// Starting projection name.
        from: String,
        /// Target projection name.
        to: String,
        /// Transition factor in `0..=1`.
        transition: f64,
    },
    /// Interpolated `{ from, to, transition }` object produced between `interpolate` stops.
    Interpolated {
        /// Starting projection name.
        from: String,
        /// Target projection name.
        to: String,
        /// Transition factor in `0..=1`.
        transition: f64,
    },
}

impl ProjectionDefinition {
    fn target_name(&self) -> &str {
        match self {
            Self::Named(name)
            | Self::Tuple { to: name, .. }
            | Self::Interpolated { to: name, .. } => name,
        }
    }

    /// Interpolates between two projection stops at blend factor `t`.
    pub fn interpolate(&self, to: &Self, t: f64) -> Self {
        Self::Interpolated {
            from: self.target_name().to_owned(),
            to: to.target_name().to_owned(),
            transition: t,
        }
    }

    /// Serializes to the GL JS representation expected by the expression conformance suite.
    pub fn to_json(&self) -> Json {
        match self {
            Self::Named(name) => Json::String(name.clone()),
            Self::Tuple {
                from,
                to,
                transition,
            } => json!([from, to, number_to_json(*transition)]),
            Self::Interpolated {
                from,
                to,
                transition,
            } => json!({
                "from": from,
                "to": to,
                "transition": number_to_json(*transition),
            }),
        }
    }
}

/// Alternating anchor name and `[x, y]` em-offset pairs (`variableAnchorOffsetCollection`).
#[derive(Clone, Debug, PartialEq)]
pub struct VariableAnchorOffsetCollection(pub Vec<(String, [f64; 2])>);

impl VariableAnchorOffsetCollection {
    /// Componentwise offset interpolation when both collections list identical anchors in order.
    pub fn interpolate(&self, to: &Self, t: f64) -> Option<Self> {
        if self.0.len() != to.0.len() {
            return None;
        }
        let mut pairs = Vec::with_capacity(self.0.len());
        for ((anchor_a, offset_a), (anchor_b, offset_b)) in self.0.iter().zip(&to.0) {
            if anchor_a != anchor_b {
                return None;
            }
            pairs.push((
                anchor_a.clone(),
                interpolate_offset_pair(*offset_a, *offset_b, t),
            ));
        }
        Some(Self(pairs))
    }

    /// Serializes to `{ "values": [anchor, [x, y], ...] }`.
    pub fn to_json(&self) -> Json {
        let mut values = Vec::with_capacity(self.0.len() * 2);
        for (anchor, [x, y]) in &self.0 {
            values.push(Json::String(anchor.clone()));
            values.push(json!([number_to_json(*x), number_to_json(*y)]));
        }
        values_json(values)
    }
}

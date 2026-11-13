//! Composite style-spec value types: `padding`, `numberArray`, `colorArray`,
//! `projectionDefinition`, and `variableAnchorOffsetCollection`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use super::{Color, Value};
use crate::style::expression::interpolation::{interpolate_number, ColorSpace};

/// Valid symbol anchor names accepted by `variableAnchorOffsetCollection`.
const VALID_ANCHORS: &[&str] = &[
    "center",
    "left",
    "right",
    "top",
    "bottom",
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
];

fn is_valid_anchor(anchor: &str) -> bool {
    VALID_ANCHORS.contains(&anchor)
}

/// Directional `[top, right, bottom, left]` box padding (`padding`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Padding(pub [f64; 4]);

impl Padding {
    /// Parses a runtime value into `[top, right, bottom, left]` CSS-shorthand padding.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Padding(padding) => Some(*padding),
            Value::Number(n) if n.is_finite() => Some(Self([*n, *n, *n, *n])),
            Value::Array(items) if (1..=4).contains(&items.len()) => {
                let mut nums = [0.0; 4];
                for (idx, item) in items.iter().enumerate() {
                    let n = item.as_number().filter(|v| v.is_finite())?;
                    nums[idx] = n;
                }
                Some(Self(match items.len() {
                    1 => [nums[0], nums[0], nums[0], nums[0]],
                    2 => [nums[0], nums[1], nums[0], nums[1]],
                    3 => [nums[0], nums[1], nums[2], nums[1]],
                    _ => nums,
                }))
            }
            _ => None,
        }
    }

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
        json!({
            "values": self.0.map(|v| Value::Number(v).to_json())
        })
    }
}

/// Variable-length numeric array (`numberArray`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberArray(pub Vec<f64>);

impl NumberArray {
    /// Parses a single number or homogeneous number array into a [`NumberArray`].
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::NumberArray(arr) => Some(arr.clone()),
            Value::Number(n) if n.is_finite() => Some(Self(vec![*n])),
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_number().filter(|v| v.is_finite()))
                .collect::<Option<Vec<f64>>>()
                .map(Self),
            _ => None,
        }
    }

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
        json!({
            "values": self.0.iter().map(|v| Value::Number(*v).to_json()).collect::<Vec<_>>()
        })
    }
}

/// Variable-length colour array (`colorArray`).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorArray(pub Vec<Color>);

impl ColorArray {
    /// Parses a colour, CSS colour string, or array of colours/strings into a [`ColorArray`].
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::ColorArray(arr) => Some(arr.clone()),
            Value::Color(color) => Some(Self(vec![*color])),
            Value::String(text) => Color::parse(text).map(|color| Self(vec![color])),
            Value::Array(items) => items
                .iter()
                .map(|item| match item {
                    Value::Color(color) => Some(*color),
                    Value::String(text) => Color::parse(text),
                    _ => None,
                })
                .collect::<Option<Vec<Color>>>()
                .map(Self),
            _ => None,
        }
    }

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
        json!({
            "values": self.0.iter().map(|color| {
                let [r, g, b, a] = color.premultiplied();
                json!({
                    "r": Value::Number(r).to_json(),
                    "g": Value::Number(g).to_json(),
                    "b": Value::Number(b).to_json(),
                    "a": Value::Number(a).to_json(),
                })
            }).collect::<Vec<_>>()
        })
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
    /// Parses a projection name string, `[from, to, t]` tuple, or `{ from, to, transition }`.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::ProjectionDefinition(proj) => Some(proj.clone()),
            Value::String(name) if !name.is_empty() => Some(Self::Named(name.clone())),
            Value::Array(items) if items.len() == 3 => {
                let from = items[0].as_str().filter(|s| !s.is_empty())?;
                let to = items[1].as_str().filter(|s| !s.is_empty())?;
                let transition = items[2].as_number().filter(|n| n.is_finite())?;
                Some(Self::Tuple {
                    from: from.to_owned(),
                    to: to.to_owned(),
                    transition,
                })
            }
            Value::Object(members) => {
                let from = members.get("from")?.as_str().filter(|s| !s.is_empty())?;
                let to = members.get("to")?.as_str().filter(|s| !s.is_empty())?;
                let transition = members
                    .get("transition")?
                    .as_number()
                    .filter(|n| n.is_finite())?;
                Some(Self::Interpolated {
                    from: from.to_owned(),
                    to: to.to_owned(),
                    transition,
                })
            }
            _ => None,
        }
    }

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
            } => json!([from, to, Value::Number(*transition).to_json()]),
            Self::Interpolated {
                from,
                to,
                transition,
            } => json!({
                "from": from,
                "to": to,
                "transition": Value::Number(*transition).to_json(),
            }),
        }
    }
}

/// Alternating anchor name and `[x, y]` em-offset pairs (`variableAnchorOffsetCollection`).
#[derive(Clone, Debug, PartialEq)]
pub struct VariableAnchorOffsetCollection(pub Vec<(String, [f64; 2])>);

impl VariableAnchorOffsetCollection {
    /// Parses a non-empty alternating `[anchor, [x, y], ...]` array.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::VariableAnchorOffsetCollection(coll) => Some(coll.clone()),
            Value::Array(items) if !items.is_empty() && items.len().is_multiple_of(2) => {
                let mut pairs = Vec::with_capacity(items.len() / 2);
                for [anchor_val, offset_val] in items.as_chunks::<2>().0 {
                    let anchor = anchor_val.as_str().filter(|a| is_valid_anchor(a))?;
                    let Value::Array(xy) = offset_val else {
                        return None;
                    };
                    if xy.len() != 2 {
                        return None;
                    }
                    let x = xy[0].as_number().filter(|n| n.is_finite())?;
                    let y = xy[1].as_number().filter(|n| n.is_finite())?;
                    pairs.push((anchor.to_owned(), [x, y]));
                }
                Some(Self(pairs))
            }
            _ => None,
        }
    }

    /// Componentwise offset interpolation when both collections list identical anchors in order.
    pub fn interpolate(&self, to: &Self, t: f64) -> Option<Self> {
        if self.0.len() != to.0.len() {
            return None;
        }
        let mut pairs = Vec::with_capacity(self.0.len());
        for ((anchor_a, [x1, y1]), (anchor_b, [x2, y2])) in self.0.iter().zip(&to.0) {
            if anchor_a != anchor_b {
                return None;
            }
            pairs.push((
                anchor_a.clone(),
                [
                    interpolate_number(*x1, *x2, t),
                    interpolate_number(*y1, *y2, t),
                ],
            ));
        }
        Some(Self(pairs))
    }

    /// Serializes to `{ "values": [anchor, [x, y], ...] }`.
    pub fn to_json(&self) -> Json {
        let mut values = Vec::with_capacity(self.0.len() * 2);
        for (anchor, [x, y]) in &self.0 {
            values.push(Json::String(anchor.clone()));
            values.push(json!([
                Value::Number(*x).to_json(),
                Value::Number(*y).to_json(),
            ]));
        }
        json!({ "values": values })
    }
}

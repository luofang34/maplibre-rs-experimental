//! Conversions from runtime expression [`Value`] into composite style-spec types.

use super::{
    Color, ColorArray, NumberArray, Padding, ProjectionDefinition, Value,
    VariableAnchorOffsetCollection,
};

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

fn padding_from_shorthand(len: usize, nums: [f64; 4]) -> Padding {
    let [a, b, c, d] = nums;
    Padding(match len {
        1 => [a, a, a, a],
        2 => [a, b, a, b],
        3 => [a, b, c, b],
        _ => [a, b, c, d],
    })
}

impl Value {
    fn as_finite_number(&self) -> Option<f64> {
        self.as_number().filter(|n| n.is_finite())
    }

    fn as_non_empty_str(&self) -> Option<&str> {
        self.as_str().filter(|s| !s.is_empty())
    }

    /// Parses this value into `[top, right, bottom, left]` CSS-shorthand box padding.
    pub fn to_padding(&self) -> Option<Padding> {
        match self {
            Self::Padding(padding) => Some(*padding),
            Self::Number(n) if n.is_finite() => Some(Padding([*n, *n, *n, *n])),
            Self::Array(items) if (1..=4).contains(&items.len()) => {
                let mut nums = [0.0; 4];
                for (slot, item) in nums.iter_mut().zip(items.iter()) {
                    *slot = item.as_finite_number()?;
                }
                Some(padding_from_shorthand(items.len(), nums))
            }
            _ => None,
        }
    }

    /// Parses this value (single number or homogeneous number array) into a [`NumberArray`].
    pub fn to_number_array(&self) -> Option<NumberArray> {
        match self {
            Self::NumberArray(arr) => Some(arr.clone()),
            Self::Number(n) if n.is_finite() => Some(NumberArray(vec![*n])),
            Self::Array(items) => items
                .iter()
                .map(Self::as_finite_number)
                .collect::<Option<Vec<f64>>>()
                .map(NumberArray),
            _ => None,
        }
    }

    /// Parses this value (colour, CSS colour string, or array thereof) into a [`ColorArray`].
    pub fn to_color_array(&self) -> Option<ColorArray> {
        match self {
            Self::ColorArray(arr) => Some(arr.clone()),
            Self::Color(color) => Some(ColorArray(vec![*color])),
            Self::String(text) => Color::parse(text).map(|color| ColorArray(vec![color])),
            Self::Array(items) => items
                .iter()
                .map(|item| match item {
                    Self::Color(color) => Some(*color),
                    Self::String(text) => Color::parse(text),
                    _ => None,
                })
                .collect::<Option<Vec<Color>>>()
                .map(ColorArray),
            _ => None,
        }
    }

    /// Parses this value (projection name, `[from, to, t]`, or `{ from, to, transition }`) into
    /// a [`ProjectionDefinition`].
    pub fn to_projection_definition(&self) -> Option<ProjectionDefinition> {
        match self {
            Self::ProjectionDefinition(proj) => Some(proj.clone()),
            Self::String(name) if !name.is_empty() => {
                Some(ProjectionDefinition::Named(name.clone()))
            }
            Self::Array(items) => match items.as_slice() {
                [from, to, transition] => Some(ProjectionDefinition::Tuple {
                    from: from.as_non_empty_str()?.to_owned(),
                    to: to.as_non_empty_str()?.to_owned(),
                    transition: transition.as_finite_number()?,
                }),
                _ => None,
            },
            Self::Object(members) => Some(ProjectionDefinition::Interpolated {
                from: members.get("from")?.as_non_empty_str()?.to_owned(),
                to: members.get("to")?.as_non_empty_str()?.to_owned(),
                transition: members.get("transition")?.as_finite_number()?,
            }),
            _ => None,
        }
    }

    /// Parses this value (non-empty alternating `[anchor, [x, y], ...]`) into a
    /// [`VariableAnchorOffsetCollection`].
    pub fn to_variable_anchor_offset_collection(&self) -> Option<VariableAnchorOffsetCollection> {
        match self {
            Self::VariableAnchorOffsetCollection(coll) => Some(coll.clone()),
            Self::Array(items) if !items.is_empty() && items.len().is_multiple_of(2) => {
                let mut pairs = Vec::with_capacity(items.len() / 2);
                for [anchor_val, offset_val] in items.as_chunks::<2>().0 {
                    let anchor = anchor_val.as_str().filter(|a| is_valid_anchor(a))?;
                    let Self::Array(xy) = offset_val else {
                        return None;
                    };
                    let [x_val, y_val] = xy.as_slice() else {
                        return None;
                    };
                    pairs.push((
                        anchor.to_owned(),
                        [x_val.as_finite_number()?, y_val.as_finite_number()?],
                    ));
                }
                Some(VariableAnchorOffsetCollection(pairs))
            }
            _ => None,
        }
    }
}

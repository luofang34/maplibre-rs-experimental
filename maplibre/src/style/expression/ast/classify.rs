//! Static output-type and constantness classification for [`Expression`] trees.

use super::{Coercion, Expression, FeatureProperty, Global};
use crate::style::expression::value::Type;

impl Expression {
    /// The type the expression evaluates to, as far as it is known before evaluation.
    pub fn output_type(&self) -> Type {
        match self {
            Self::Literal(value) => value.type_of(),
            Self::Folded { output, .. } => output.clone(),
            Self::Error(_) => Type::Error,
            Self::Global(_) => Type::Number,
            Self::Feature(FeatureProperty::Id) | Self::Accumulated => Type::Value,
            Self::Feature(FeatureProperty::GeometryType) => Type::String,
            Self::Feature(FeatureProperty::Properties) => Type::Object,
            Self::Get { .. } | Self::GlobalState(_) => Type::Value,
            Self::Has { .. }
            | Self::Compare { .. }
            | Self::All(_)
            | Self::Any(_)
            | Self::Not(_)
            | Self::IsSupportedScript(_)
            | Self::Within(_)
            | Self::In { .. } => Type::Boolean,
            Self::Distance(_) => Type::Number,
            Self::Collator { .. } => Type::Collator,
            Self::Var { bound, .. } => bound.output_type(),
            Self::Let { body, .. } => body.output_type(),
            Self::Case { output, .. }
            | Self::Match { output, .. }
            | Self::Coalesce { output, .. }
            | Self::Slice { output, .. }
            | Self::At { output, .. }
            | Self::Interpolate { output, .. }
            | Self::Step { output, .. } => output.clone(),
            Self::IndexOf { .. }
            | Self::Length(_)
            | Self::Arithmetic { .. }
            | Self::Math { .. }
            | Self::MinMax { .. } => Type::Number,
            Self::TypeOf(_)
            | Self::Concat(_)
            | Self::Join { .. }
            | Self::StringCase { .. }
            | Self::ResolvedLocale(_)
            | Self::NumberFormat { .. } => Type::String,
            Self::Split { .. } => Type::array(Type::String, None),
            Self::Image(_) => Type::ResolvedImage,
            Self::Format(_) => Type::Formatted,
            Self::Assert { required, .. } => required.clone(),
            Self::Coerce { coercion, .. } => match coercion {
                Coercion::Number => Type::Number,
                Coercion::String => Type::String,
                Coercion::Boolean => Type::Boolean,
                Coercion::Color => Type::Color,
                Coercion::Formatted => Type::Formatted,
                Coercion::ResolvedImage => Type::ResolvedImage,
                Coercion::Padding => Type::Padding,
                Coercion::NumberArray => Type::NumberArray,
                Coercion::ColorArray => Type::ColorArray,
                Coercion::ProjectionDefinition => Type::ProjectionDefinition,
                Coercion::VariableAnchorOffsetCollection => Type::VariableAnchorOffsetCollection,
            },
            Self::ToRgba(_) => Type::array(Type::Number, Some(4)),
            Self::Rgba(_) => Type::Color,
        }
    }

    /// Whether the result is the same for every feature.
    pub fn is_feature_constant(&self) -> bool {
        match self {
            Self::Feature(_) | Self::Within(_) | Self::Distance(_) | Self::Accumulated => false,
            Self::Get { object: None, .. } | Self::Has { object: None, .. } => false,
            _ => self.children_all(Self::is_feature_constant),
        }
    }

    /// Whether the result is the same at every zoom.
    pub fn is_zoom_constant(&self) -> bool {
        match self {
            Self::Global(Global::Zoom) => false,
            _ => self.children_all(Self::is_zoom_constant),
        }
    }

    /// Whether the result depends on neither the feature nor the map.
    pub fn is_constant(&self) -> bool {
        match self {
            // Whether an image exists is known only where it is drawn.
            Self::Global(_) | Self::GlobalState(_) | Self::Image(_) => false,
            _ => self.is_feature_constant() && self.children_all(Self::is_constant),
        }
    }

    fn children_all(&self, predicate: fn(&Expression) -> bool) -> bool {
        let mut all = true;
        self.for_each_child(&mut |child| {
            if all && !predicate(child) {
                all = false;
            }
        });
        all
    }
}

//! Symbol properties consumed by layout, placement and shader uniforms.

use serde_json::Value;

use super::{
    Evaluation::{Feature, Zoom},
    LayerValidation,
};
use crate::style::{
    layer::SymbolPaint,
    property::{ImageName, StyleProperty, TextField},
    symbol::{is_layout, property, SymbolEval, SymbolPropertySpec as Property},
};

impl LayerValidation<'_> {
    pub(super) fn symbol(&mut self, paint: &SymbolPaint) {
        self.property("layout.text-field", paint.text_field.as_ref(), Feature);
        self.property("layout.text-size", paint.text_size.as_ref(), Feature);
        for (name, value) in &paint.properties {
            let scope = if is_layout(name) { "layout" } else { "paint" };
            let path = format!("{scope}.{name}");
            let map_eval = |eval| match eval {
                SymbolEval::Feature => Feature,
                SymbolEval::Zoom => Zoom,
            };
            match property(name) {
                Some(Property::Color) => self.property(&path, Some(&StyleProperty::<csscolorparser::Color>::parse(value)), Feature),
                Some(Property::Pair(evaluation)) => self.property(&path, Some(&StyleProperty::<crate::style::translation::Pair>::parse(value)), map_eval(evaluation)),
                Some(Property::Number(evaluation)) => self.property(&path, Some(&StyleProperty::<f32>::parse(value)), map_eval(evaluation)),
                Some(Property::Text) => self.property(&path, Some(&StyleProperty::<TextField>::parse(value)), Feature),
                Some(Property::Image) => self.property(&path, Some(&StyleProperty::<ImageName>::parse(value)), Feature),
                Some(Property::BooleanExpression) => self.property(&path, Some(&StyleProperty::<bool>::parse(value)), Feature),
                Some(kind) if valid_literal(&kind, value) => {}
                // A string that is none of the values is no expression either.
                Some(Property::FeatureEnum(_)) if !value.is_string() => self.property(&path, Some(&StyleProperty::<TextField>::parse(value)), Feature),
                // These follow the zoom; `SymbolPaint::layout_enum` evaluates them.
                Some(Property::Enum(_)) if !value.is_string() && follows_zoom(name) => {
                    self.property(&path, Some(&StyleProperty::<TextField>::parse(value)), Zoom)
                }
                Some(_) => self.unsupported(&path, "this property requires a supported literal value; expressions are not evaluated"),
                None => self.unsupported(&path, "property is not implemented"),
            }
        }
    }
}

fn follows_zoom(name: &str) -> bool {
    use crate::style::symbol::{ZOOM_ONLY_ENUMS, ZOOM_ONLY_PAINT_ENUMS};
    ZOOM_ONLY_ENUMS.contains(&name) || ZOOM_ONLY_PAINT_ENUMS.contains(&name)
}

fn valid_literal(property: &Property, value: &Value) -> bool {
    match property {
        Property::Boolean => value.is_boolean(),
        Property::AnchorOffsets => value.as_array().is_some_and(|items| {
            items.len() % 2 == 0
                && items.as_chunks::<2>().0.iter().all(|pair| {
                    pair[0].as_str().is_some()
                        && pair[1].as_array().is_some_and(|offset| {
                            offset.len() == 2 && offset.iter().all(Value::is_number)
                        })
                })
        }),
        Property::Anchors => value.as_array().is_some_and(|items| {
            items.iter().all(|item| {
                item.as_str().is_some_and(|anchor| {
                    matches!(
                        anchor,
                        "center"
                            | "left"
                            | "right"
                            | "top"
                            | "bottom"
                            | "top-left"
                            | "top-right"
                            | "bottom-left"
                            | "bottom-right"
                    )
                })
            })
        }),
        Property::WritingModes => value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| matches!(item.as_str(), Some("horizontal" | "vertical")))
        }),
        Property::Padding => value
            .as_array()
            .is_some_and(|items| items.len() == 4 && items.iter().all(Value::is_number)),
        Property::Font => value.as_array().is_some_and(|items| {
            !items.is_empty()
                && !crate::style::expression::is_expression(items)
                && items.iter().all(Value::is_string)
        }),
        Property::Enum(allowed) | Property::FeatureEnum(allowed) => {
            value.as_str().is_some_and(|value| allowed.contains(&value))
        }
        _ => false,
    }
}

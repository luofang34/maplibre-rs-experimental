//! Symbol properties consumed by layout, placement and shader uniforms.

use serde_json::Value;

use super::{
    Evaluation::{Feature, Zoom},
    LayerValidation,
};
use crate::style::{
    layer::SymbolPaint,
    property::{StyleProperty, TextField},
};

enum Property {
    Color,
    Number(super::Evaluation),
    Text,
    Boolean,
    BooleanExpression,
    Offset,
    Font,
    Enum(&'static [&'static str]),
}

fn property(name: &str) -> Option<Property> {
    Some(match name {
        "text-color" | "text-halo-color" | "icon-color" | "icon-halo-color" => Property::Color,
        "text-opacity" | "text-halo-width" | "text-halo-blur" | "icon-opacity"
        | "icon-halo-width" | "icon-halo-blur" | "text-size" | "icon-size" | "text-padding"
        | "icon-padding" | "symbol-spacing" | "text-max-angle" => Property::Number(Zoom),
        "text-max-width"
        | "text-line-height"
        | "text-letter-spacing"
        | "symbol-sort-key"
        | "text-rotate"
        | "icon-rotate"
        | "symbol-height-offset"
        | "text-height-offset"
        | "icon-height-offset" => Property::Number(Feature),
        "text-field" | "icon-image" | "text-anchor" | "icon-anchor" | "text-justify" => {
            Property::Text
        }
        "text-allow-overlap"
        | "icon-allow-overlap"
        | "text-ignore-placement"
        | "icon-ignore-placement"
        | "text-optional"
        | "icon-optional" => Property::BooleanExpression,
        "text-overlap" | "icon-overlap" => Property::Enum(&["never", "always", "cooperative"]),
        "text-keep-upright" | "icon-keep-upright" => Property::Boolean,
        "text-offset" | "icon-offset" => Property::Offset,
        "text-font" => Property::Font,
        "text-transform" => Property::Enum(&["none", "uppercase", "lowercase"]),
        "symbol-placement" => Property::Enum(&["point", "line", "line-center"]),
        "text-pitch-alignment"
        | "icon-pitch-alignment"
        | "text-rotation-alignment"
        | "icon-rotation-alignment" => Property::Enum(&["auto", "map", "viewport"]),
        "symbol-height-anchor" => Property::Enum(&["ground", "absolute"]),
        "text-height-anchor" | "icon-height-anchor" => {
            Property::Enum(&["ground", "sea", "absolute"])
        }
        _ => return None,
    })
}

pub(crate) fn supports(name: &str) -> bool {
    property(name).is_some()
}

pub(crate) fn is_layout(name: &str) -> bool {
    supports(name)
        && !matches!(
            name,
            "text-color"
                | "text-halo-color"
                | "icon-color"
                | "icon-halo-color"
                | "text-opacity"
                | "text-halo-width"
                | "text-halo-blur"
                | "icon-opacity"
                | "icon-halo-width"
                | "icon-halo-blur"
                | "text-height-offset"
                | "text-height-anchor"
                | "icon-height-offset"
                | "icon-height-anchor"
        )
}

impl LayerValidation<'_> {
    pub(super) fn symbol(&mut self, paint: &SymbolPaint) {
        self.property("layout.text-field", paint.text_field.as_ref(), Feature);
        self.property("layout.text-size", paint.text_size.as_ref(), Zoom);
        for (name, value) in &paint.properties {
            let scope = if is_layout(name) { "layout" } else { "paint" };
            let path = format!("{scope}.{name}");
            match property(name) {
                Some(Property::Color) => self.property(&path, Some(&StyleProperty::<csscolorparser::Color>::parse(value)), Zoom),
                Some(Property::Number(evaluation)) => self.property(&path, Some(&StyleProperty::<f32>::parse(value)), evaluation),
                Some(Property::Text) => self.property(&path, Some(&StyleProperty::<TextField>::parse(value)), Feature),
                Some(Property::BooleanExpression) => self.property(&path, Some(&StyleProperty::<bool>::parse(value)), Feature),
                Some(kind) if valid_literal(&kind, value) => {}
                Some(_) => self.unsupported(&path, "this property requires a supported literal value; expressions are not evaluated"),
                None => self.unsupported(&path, "property is not implemented"),
            }
        }
    }
}

fn valid_literal(property: &Property, value: &Value) -> bool {
    match property {
        Property::Boolean => value.is_boolean(),
        Property::Offset => value
            .as_array()
            .is_some_and(|items| items.len() == 2 && items.iter().all(Value::is_number)),
        Property::Font => value.as_array().is_some_and(|items| {
            !items.is_empty()
                && !crate::style::expression::is_expression(items)
                && items.iter().all(Value::is_string)
        }),
        Property::Enum(allowed) => value.as_str().is_some_and(|value| allowed.contains(&value)),
        _ => false,
    }
}

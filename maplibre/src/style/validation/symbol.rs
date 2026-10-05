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
    Image,
    Boolean,
    BooleanExpression,
    Pair(super::Evaluation),
    Padding,
    Anchors,
    AnchorOffsets,
    WritingModes,
    Font,
    Enum(&'static [&'static str]),
}

fn property(name: &str) -> Option<Property> {
    Some(match name {
        "text-color" | "text-halo-color" | "icon-color" | "icon-halo-color" => Property::Color,
        "text-opacity" | "text-halo-width" | "text-halo-blur" | "icon-opacity"
        | "icon-halo-width" | "icon-halo-blur" | "text-size" | "icon-size" => {
            Property::Number(Feature)
        }
        "text-padding" | "icon-padding" | "symbol-spacing" | "text-max-angle" => {
            Property::Number(Zoom)
        }
        "text-max-width"
        | "text-line-height"
        | "text-letter-spacing"
        | "symbol-sort-key"
        | "text-rotate"
        | "icon-rotate"
        | "text-radial-offset"
        | "symbol-height-offset"
        | "text-height-offset"
        | "icon-height-offset" => Property::Number(Feature),
        "text-field" | "text-anchor" | "icon-anchor" | "text-justify" => Property::Text,
        "icon-image" => Property::Image,
        "text-allow-overlap"
        | "icon-allow-overlap"
        | "text-ignore-placement"
        | "icon-ignore-placement"
        | "text-optional"
        | "icon-optional" => Property::BooleanExpression,
        "text-overlap" | "icon-overlap" => Property::Enum(&["never", "always", "cooperative"]),
        "text-keep-upright" | "icon-keep-upright" => Property::Boolean,
        "text-offset" | "icon-offset" => Property::Pair(Feature),
        "text-font" => Property::Font,
        "text-transform" => Property::Enum(&["none", "uppercase", "lowercase"]),
        "symbol-placement" => Property::Enum(&["point", "line", "line-center"]),
        "text-pitch-alignment" | "icon-pitch-alignment" | "icon-rotation-alignment" => {
            Property::Enum(&["auto", "map", "viewport"])
        }
        "text-rotation-alignment" => Property::Enum(&["auto", "map", "viewport", "viewport-glyph"]),
        "symbol-height-anchor" => Property::Enum(&["ground", "absolute"]),
        "text-translate" | "icon-translate" => Property::Pair(Zoom),
        "text-translate-anchor" | "icon-translate-anchor" => Property::Enum(&["map", "viewport"]),
        "text-variable-anchor" => Property::Anchors,
        "text-variable-anchor-offset" => Property::AnchorOffsets,
        "text-writing-mode" => Property::WritingModes,
        "symbol-z-order" => Property::Enum(&["auto", "viewport-y", "source"]),
        "icon-text-fit" => Property::Enum(&["none", "width", "height", "both"]),
        "icon-text-fit-padding" => Property::Padding,
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
                | "text-translate"
                | "icon-translate"
                | "text-translate-anchor"
                | "icon-translate-anchor"
                | "text-height-offset"
                | "text-height-anchor"
                | "icon-height-offset"
                | "icon-height-anchor"
        )
}

impl LayerValidation<'_> {
    pub(super) fn symbol(&mut self, paint: &SymbolPaint) {
        self.property("layout.text-field", paint.text_field.as_ref(), Feature);
        self.property("layout.text-size", paint.text_size.as_ref(), Feature);
        for (name, value) in &paint.properties {
            let scope = if is_layout(name) { "layout" } else { "paint" };
            let path = format!("{scope}.{name}");
            match property(name) {
                Some(Property::Color) => self.property(&path, Some(&StyleProperty::<csscolorparser::Color>::parse(value)), Feature),
                Some(Property::Pair(evaluation)) => self.property(&path, Some(&StyleProperty::<crate::style::translation::Pair>::parse(value)), evaluation),
                Some(Property::Number(evaluation)) => self.property(&path, Some(&StyleProperty::<f32>::parse(value)), evaluation),
                Some(Property::Text) => self.property(&path, Some(&StyleProperty::<TextField>::parse(value)), Feature),
                Some(Property::Image) => self.property(&path, Some(&StyleProperty::<crate::style::layer::ImageName>::parse(value)), Feature),
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
        Property::Enum(allowed) => value.as_str().is_some_and(|value| allowed.contains(&value)),
        _ => false,
    }
}

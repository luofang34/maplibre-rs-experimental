//! Evaluated symbol layout and paint shared by glyph preparation and rendering.
use crate::style::{
    expression::FeatureProperties,
    layer::{StyleProperty, SymbolPaint, TextField},
};

impl SymbolPaint {
    /// Evaluates a numeric symbol property.
    pub fn number(
        &self,
        name: &str,
        properties: &FeatureProperties,
        zoom: f64,
        fallback: f32,
    ) -> f32 {
        self.properties
            .get(name)
            .and_then(|value| StyleProperty::<f32>::parse(value).evaluate_for(properties, zoom))
            .filter(|value| value.is_finite())
            .unwrap_or(fallback)
    }

    /// Evaluates a string or token template property.
    pub fn text(&self, name: &str, properties: &FeatureProperties, zoom: f64) -> Option<String> {
        let field = if name == "text-field" {
            self.text_field.clone()
        } else {
            self.properties
                .get(name)
                .map(StyleProperty::<TextField>::parse)
        };
        field
            .and_then(|value| value.evaluate_for(properties, zoom))
            .map(|text| text.0)
    }

    /// Font stack requested by the style.
    pub fn font_stack(&self) -> String {
        self.properties
            .get("text-font")
            .and_then(|value| value.as_array())
            .map(|fonts| {
                fonts
                    .iter()
                    .filter_map(|font| font.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|font| !font.is_empty())
            .unwrap_or_else(|| "Open Sans Regular".to_string())
    }

    /// Text transformed before both glyph requests and layout.
    pub fn label(&self, properties: &FeatureProperties, zoom: f64) -> Option<String> {
        let text = self.text("text-field", properties, zoom)?;
        let text = match self
            .properties
            .get("text-transform")
            .and_then(|value| value.as_str())
        {
            Some("uppercase") => text.to_uppercase(),
            Some("lowercase") => text.to_lowercase(),
            _ => text,
        };
        let shaped =
            crate::legacy::bidi::apply_arabic_shaping(&widestring::U16String::from(text.as_str()));
        let text = shaped.to_string_lossy().trim().to_string();
        (!text.trim().is_empty()).then_some(text)
    }
}

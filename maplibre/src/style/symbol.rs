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

    /// Evaluates a string or token template property that may name images, which `image`
    /// expressions in it are checked against.
    pub fn text_among_images(
        &self,
        name: &str,
        properties: &FeatureProperties,
        zoom: f64,
        images: &dyn crate::style::expression::ImageSet,
    ) -> Option<String> {
        let property = self
            .properties
            .get(name)
            .map(StyleProperty::<TextField>::parse)?;
        let context = crate::style::expression::EvaluationContext {
            available_images: Some(images),
            ..crate::style::expression::EvaluationContext::for_feature(zoom, properties)
        };
        property.evaluate(&context).map(|text| text.0)
    }

    /// The names of the images the layer's `icon-image` can ask for with `image` expressions.
    pub fn icon_image_names(&self) -> Vec<String> {
        fn collect(value: &serde_json::Value, names: &mut Vec<String>) {
            let Some(items) = value.as_array() else {
                return;
            };
            if let [serde_json::Value::String(operator), serde_json::Value::String(name)] =
                items.as_slice()
            {
                if operator == "image" {
                    names.push(name.clone());
                }
            }
            items.iter().for_each(|item| collect(item, names));
        }
        let mut names = Vec::new();
        if let Some(value) = self.properties.get("icon-image") {
            collect(value, &mut names);
        }
        names
    }

    /// Whether shared symbol height is evaluated during placement rather than tile layout.
    pub fn uses_shared_height(&self) -> bool {
        self.properties.contains_key("symbol-height-offset")
    }

    /// Evaluates the shared point-symbol height, accepting component offsets as a fallback.
    pub fn height_offset(&self, component: &str, properties: &FeatureProperties, zoom: f64) -> f32 {
        let name = if self.properties.contains_key("symbol-height-offset") {
            "symbol-height-offset".to_string()
        } else {
            format!("{component}-height-offset")
        };
        self.number(&name, properties, zoom, 0.0)
    }

    /// Whether a symbol's height includes the terrain elevation below its anchor.
    pub fn height_follows_ground(&self, component: &str) -> bool {
        self.properties
            .get("symbol-height-anchor")
            .or_else(|| self.properties.get(&format!("{component}-height-anchor")))
            .and_then(|value| value.as_str())
            .unwrap_or("ground")
            == "ground"
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
        let (text, _) = self.shaped_field(properties, zoom)?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// The text field with `text-transform` applied and the Arabic of each section shaped on
    /// its own, as GL JS shapes each run of a formatted label; the sections, empty for plain
    /// text, keep the lengths their shaped runs have.
    fn shaped_field(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
    ) -> Option<(String, Vec<crate::style::property::TextSection>)> {
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
        let shape = |text: String| {
            if crate::style::arabic_shaping::needs_shaping(&text) {
                crate::style::arabic_shaping::shape(&text)
            } else {
                text
            }
        };
        let sections = self
            .text_field
            .as_ref()
            .and_then(|field| field.evaluate_for(properties, zoom))
            .map(|field| field.1)
            .unwrap_or_default();
        let total: usize = sections.iter().map(|section| section.length).sum();
        // A case change that alters the length of the text leaves no way to tell which
        // characters belong to which section.
        if sections.is_empty() || total != text.chars().count() {
            return Some((shape(text), Vec::new()));
        }
        let mut chars = text.chars();
        let mut shaped = String::with_capacity(text.len());
        let mut shaped_sections = Vec::with_capacity(sections.len());
        for section in sections {
            let run = shape(chars.by_ref().take(section.length).collect());
            shaped_sections.push(crate::style::property::TextSection {
                length: run.chars().count(),
                ..section
            });
            shaped.push_str(&run);
        }
        Some((shaped, shaped_sections))
    }
}

impl SymbolPaint {
    /// The sections of the label's text, if `text-field` is a `format` expression: the runs of
    /// `label`'s characters with their own size, colour or font; empty for one plain section.
    pub fn label_sections(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
    ) -> Vec<crate::style::property::TextSection> {
        let Some((text, sections)) = self.shaped_field(properties, zoom) else {
            return Vec::new();
        };
        if sections.is_empty() || text.trim().is_empty() {
            return Vec::new();
        }
        let leading = text.chars().take_while(|c| c.is_whitespace()).count();
        let mut skip = leading;
        let mut remaining = text.trim().chars().count();
        let mut trimmed = Vec::new();
        for mut section in sections {
            let dropped = skip.min(section.length);
            skip -= dropped;
            section.length -= dropped;
            section.length = section.length.min(remaining);
            remaining -= section.length;
            if section.length > 0 {
                trimmed.push(section);
            }
        }
        trimmed
    }
}

#[cfg(test)]
#[path = "symbol/tests.rs"]
mod tests;

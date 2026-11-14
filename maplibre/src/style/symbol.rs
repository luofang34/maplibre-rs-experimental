//! Evaluated symbol layout and paint shared by glyph preparation and rendering.

pub(crate) mod compiled_cache;
pub use compiled_cache::CompiledProperties;

/// Symbol layout enumerations whose value may follow the zoom but not the feature, per the
/// style specification's `"expression": {"parameters": ["zoom"]}`.
pub const ZOOM_ONLY_ENUMS: &[&str] = &[
    "symbol-placement",
    "text-rotation-alignment",
    "icon-rotation-alignment",
    "text-pitch-alignment",
    "icon-pitch-alignment",
    "text-overlap",
    "icon-overlap",
    "icon-text-fit",
    "symbol-z-order",
];

/// Symbol paint enumerations that may follow the zoom but not the feature. Paint follows the
/// view's zoom, so unlike [`ZOOM_ONLY_ENUMS`] a tile's layout does not settle them.
pub const ZOOM_ONLY_PAINT_ENUMS: &[&str] = &["text-translate-anchor", "icon-translate-anchor"];

use crate::style::{
    expression::FeatureProperties,
    layer::{SymbolPaint, TextField},
    property::{CompiledProperty, PropertyValue},
};

impl SymbolPaint {
    /// Returns the compiled property `name` if defined, parsing and caching it on demand.
    pub fn compiled_property<T>(&self, name: &str) -> Option<CompiledProperty<T>>
    where
        T: PropertyValue + Send + Sync + 'static,
    {
        let source = self.properties.get(name)?;
        self.compiled.get(name, source)
    }

    /// Evaluates a numeric symbol property using pre-compiled expression.
    pub fn number(
        &self,
        name: &str,
        properties: &FeatureProperties,
        zoom: f64,
        fallback: f32,
    ) -> f32 {
        self.compiled_property::<f32>(name)
            .and_then(|prop| prop.evaluate_for(properties, zoom))
            .filter(|value| value.is_finite())
            .unwrap_or(fallback)
    }

    /// Evaluates a string or token template property using pre-compiled expression.
    pub fn text(&self, name: &str, properties: &FeatureProperties, zoom: f64) -> Option<String> {
        if name == "text-field" {
            return self.field(properties, zoom, None).map(|field| field.0);
        }
        self.compiled_property::<TextField>(name)
            .and_then(|prop| prop.evaluate_for(properties, zoom))
            .map(|text| text.0)
    }

    /// The text field, evaluated against `images` when given: what an `image` in it resolves
    /// to, and so what a `coalesce` around it chooses, depends on the images the map holds.
    fn field(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
        images: Option<&dyn crate::style::expression::ImageSet>,
    ) -> Option<TextField> {
        let property = self.text_field.as_ref()?;
        let context = crate::style::expression::EvaluationContext {
            available_images: images,
            ..crate::style::expression::EvaluationContext::for_feature(zoom, properties)
        };
        property.evaluate(&context).map(|text| text.0)
    }

    /// Evaluates the image property `name`, such as `icon-image`, against the images the map
    /// holds: what an `image` expression in it resolves to, and so what a `coalesce` around it
    /// chooses, depends on them.
    pub fn text_among_images(
        &self,
        name: &str,
        properties: &FeatureProperties,
        zoom: f64,
        images: &dyn crate::style::expression::ImageSet,
    ) -> Option<String> {
        let property = self.compiled_property::<crate::style::layer::ImageName>(name)?;
        let context = crate::style::expression::EvaluationContext {
            available_images: Some(images),
            ..crate::style::expression::EvaluationContext::for_feature(zoom, properties)
        };
        match property {
            CompiledProperty::Constant(name) => Some(name.0),
            CompiledProperty::CameraFunction(curve) => {
                let val = curve.expression().evaluate(&context).ok()?;
                crate::style::layer::ImageName::from_value(&val).map(|name| name.0)
            }
            CompiledProperty::DataDriven(expr) | CompiledProperty::Composite(expr) => {
                let val = expr.evaluate(&context).ok()?;
                crate::style::layer::ImageName::from_value(&val).map(|name| name.0)
            }
        }
    }

    /// Whether shared symbol height is evaluated during placement rather than tile layout.
    pub fn uses_shared_height(&self) -> bool {
        self.properties.contains_key("symbol-height-offset")
    }

    /// The enumeration `name` at `zoom`: the literal, or the value of a zoom expression such as
    /// `["step", ["zoom"], "point", 11, "line"]`, which the style specification allows for the
    /// properties in [`ZOOM_ONLY_ENUMS`] and [`ZOOM_ONLY_PAINT_ENUMS`].
    pub fn layout_enum(&self, name: &str, zoom: f64) -> Option<String> {
        let value = self.properties.get(name)?;
        if let Some(literal) = value.as_str() {
            return Some(literal.to_owned());
        }
        if !ZOOM_ONLY_ENUMS.contains(&name) && !ZOOM_ONLY_PAINT_ENUMS.contains(&name) {
            return None;
        }
        self.text(name, &FeatureProperties::new(), zoom)
    }

    /// Replaces the zoom expressions of [`ZOOM_ONLY_ENUMS`] with their value at `zoom`, as GL JS
    /// evaluates the layout of a tile's bucket at the tile's zoom, so that what reads them as
    /// literals lays out the tile as the style asks.
    pub fn resolve_zoom_enums(&mut self, zoom: f64) {
        for name in ZOOM_ONLY_ENUMS {
            if self
                .properties
                .get(*name)
                .is_some_and(|value| !value.is_string())
            {
                match self.layout_enum(name, zoom) {
                    Some(value) => {
                        self.properties
                            .insert((*name).to_owned(), serde_json::Value::String(value));
                    }
                    None => {
                        self.properties.remove(*name);
                    }
                }
            }
        }
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

    /// Whether symbols crossing tile boundaries (`[0, 4096]`) are suppressed (`symbol-avoid-edges`).
    pub fn avoid_edges(&self) -> bool {
        self.properties
            .get("symbol-avoid-edges")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// Plain `text-transform`ed label without legacy `.pbf` presentation-form mapping, or `None`
    /// when the label is empty, carries `format` sections, or embeds inline images.
    pub fn raw_label_among(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
        images: Option<&dyn crate::style::expression::ImageSet>,
    ) -> Option<String> {
        let TextField(text, sections) = self.field(properties, zoom, images)?;
        if !sections.is_empty() {
            return None;
        }
        let text = match self.text("text-transform", properties, zoom).as_deref() {
            Some("uppercase") => text.to_uppercase(),
            Some("lowercase") => text.to_lowercase(),
            _ => text,
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.contains(crate::style::expression::FORMAT_IMAGE) {
            return None;
        }
        Some(trimmed.to_string())
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
        self.label_among(properties, zoom, None)
    }

    /// [`Self::label`] with the images the map holds, which decide what an `image` in the
    /// field resolves to.
    pub fn label_among(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
        images: Option<&dyn crate::style::expression::ImageSet>,
    ) -> Option<String> {
        let (text, _) = self.shaped_field(properties, zoom, images)?;
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
        images: Option<&dyn crate::style::expression::ImageSet>,
    ) -> Option<(String, Vec<crate::style::property::TextSection>)> {
        let TextField(text, sections) = self.field(properties, zoom, images)?;
        // The case may vary by feature, as the style specification allows.
        let text = match self.text("text-transform", properties, zoom).as_deref() {
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
        self.label_sections_among(properties, zoom, None)
    }

    /// [`Self::label_sections`] with the images the map holds.
    pub fn label_sections_among(
        &self,
        properties: &FeatureProperties,
        zoom: f64,
        images: Option<&dyn crate::style::expression::ImageSet>,
    ) -> Vec<crate::style::property::TextSection> {
        let Some((text, sections)) = self.shaped_field(properties, zoom, images) else {
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

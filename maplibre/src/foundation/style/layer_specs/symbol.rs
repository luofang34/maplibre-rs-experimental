//! Evaluated symbol layout and paint shared by glyph preparation and rendering.

use std::{
    any::{Any, TypeId},
    sync::Mutex,
};

use rustc_hash::FxHashMap as HashMap;

use crate::style::{
    expression::FeatureProperties,
    property::{
        CompiledProperty, FormattedText, ImageName, PropertyValue, StyleProperty, TextField,
    },
};

mod shaping;

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

struct CompiledEntry {
    kind: TypeId,
    source: serde_json::Value,
    property: Box<dyn Any + Send + Sync>,
}

/// Thread-safe cache of pre-compiled properties keyed by name and type.
#[derive(Default)]
pub struct CompiledProperties(Mutex<HashMap<String, Vec<CompiledEntry>>>);

impl Clone for CompiledProperties {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for CompiledProperties {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledProperties")
    }
}

impl CompiledProperties {
    /// Returns the compiled property `name` if already compiled and `source` matches,
    /// or compiles and stores it in the cache.
    pub fn get<T>(&self, name: &str, source: &serde_json::Value) -> Option<CompiledProperty<T>>
    where
        T: PropertyValue + Send + Sync + 'static,
    {
        let kind = TypeId::of::<T>();
        let mut guard = crate::util::sync::lock_unpoisoned(&self.0);
        if let Some(entries) = guard.get_mut(name) {
            if let Some(entry) = entries.iter().find(|e| e.kind == kind) {
                if entry.source == *source {
                    if let Some(property) = entry.property.downcast_ref::<CompiledProperty<T>>() {
                        return Some(property.clone());
                    }
                }
            }
            entries.retain(|e| e.kind != kind);
        }
        let property = CompiledProperty::<T>::parse(source)?;
        let entry = CompiledEntry {
            kind,
            source: source.clone(),
            property: Box::new(property.clone()),
        };
        guard.entry(name.to_owned()).or_default().push(entry);
        Some(property)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SymbolEval {
    Feature,
    Zoom,
}

pub(crate) enum SymbolPropertySpec {
    Color,
    Number(SymbolEval),
    Text,
    Image,
    Boolean,
    BooleanExpression,
    Pair(SymbolEval),
    Padding,
    Anchors,
    AnchorOffsets,
    WritingModes,
    Font,
    Enum(&'static [&'static str]),
    FeatureEnum(&'static [&'static str]),
}

const FEATURE_NUMBER_PROPERTIES: &[&str] = &[
    "text-opacity",
    "text-halo-width",
    "text-halo-blur",
    "icon-opacity",
    "icon-halo-width",
    "icon-halo-blur",
    "text-size",
    "icon-size",
    "text-max-width",
    "text-line-height",
    "text-letter-spacing",
    "symbol-sort-key",
    "text-rotate",
    "icon-rotate",
    "text-radial-offset",
    "symbol-height-offset",
    "text-height-offset",
    "icon-height-offset",
];

pub(crate) fn property(name: &str) -> Option<SymbolPropertySpec> {
    use SymbolEval::{Feature, Zoom};
    use SymbolPropertySpec as S;
    if FEATURE_NUMBER_PROPERTIES.contains(&name) {
        return Some(S::Number(Feature));
    }
    Some(match name {
        "text-color" | "text-halo-color" | "icon-color" | "icon-halo-color" => S::Color,
        "text-padding" | "icon-padding" | "symbol-spacing" | "text-max-angle" => S::Number(Zoom),
        "text-field" | "text-anchor" | "icon-anchor" | "text-justify" => S::Text,
        "icon-image" => S::Image,
        "text-allow-overlap"
        | "icon-allow-overlap"
        | "text-ignore-placement"
        | "icon-ignore-placement"
        | "text-optional"
        | "icon-optional" => S::BooleanExpression,
        "text-overlap" | "icon-overlap" => S::Enum(&["never", "always", "cooperative"]),
        "text-keep-upright" | "icon-keep-upright" | "symbol-avoid-edges" => S::Boolean,
        "text-offset" | "icon-offset" => S::Pair(Feature),
        "text-font" => S::Font,
        "text-transform" => S::FeatureEnum(&["none", "uppercase", "lowercase"]),
        "symbol-placement" => S::Enum(&["point", "line", "line-center"]),
        "text-pitch-alignment" | "icon-pitch-alignment" | "icon-rotation-alignment" => {
            S::Enum(&["auto", "map", "viewport"])
        }
        "text-rotation-alignment" => S::Enum(&["auto", "map", "viewport", "viewport-glyph"]),
        "symbol-height-anchor" => S::Enum(&["ground", "absolute"]),
        "text-translate" | "icon-translate" => S::Pair(Zoom),
        "text-translate-anchor" | "icon-translate-anchor" => S::Enum(&["map", "viewport"]),
        "text-variable-anchor" => S::Anchors,
        "text-variable-anchor-offset" => S::AnchorOffsets,
        "text-writing-mode" => S::WritingModes,
        "symbol-z-order" => S::Enum(&["auto", "viewport-y", "source"]),
        "icon-text-fit" => S::Enum(&["none", "width", "height", "both"]),
        "icon-text-fit-padding" => S::Padding,
        "text-height-anchor" | "icon-height-anchor" => S::Enum(&["ground", "sea", "absolute"]),
        _ => return None,
    })
}

pub(crate) fn supports(name: &str) -> bool {
    property(name).is_some()
}

pub(crate) fn is_layout(name: &str) -> bool {
    supports(name) && !shaping::is_symbol_paint_name(name)
}

/// Typed text properties and additional text/icon paint and layout values.
#[derive(Debug, Clone, Default)]
pub struct SymbolPaint {
    /// Text of each symbol: a `{token}` template, a literal, or an expression; `None` draws
    /// no text.
    pub text_field: Option<StyleProperty<FormattedText>>,

    /// Text em size in screen pixels, evaluated for each feature and zoom.
    pub text_size: Option<StyleProperty<f32>>,
    /// Symbol paint and layout properties evaluated during placement.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Pre-parsed compiled properties cache.
    pub(crate) compiled: CompiledProperties,
}

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
        let property = self.compiled_property::<ImageName>(name)?;
        let context = crate::style::expression::EvaluationContext {
            available_images: Some(images),
            ..crate::style::expression::EvaluationContext::for_feature(zoom, properties)
        };
        match property {
            CompiledProperty::Constant(name) => Some(name.0),
            CompiledProperty::CameraFunction(curve) => {
                let val = curve.expression().evaluate(&context).ok()?;
                ImageName::from_value(&val).map(|name| name.0)
            }
            CompiledProperty::DataDriven(expr) | CompiledProperty::Composite(expr) => {
                let val = expr.evaluate(&context).ok()?;
                ImageName::from_value(&val).map(|name| name.0)
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
        if !shaping::is_zoom_only_enum(name) {
            return None;
        }
        self.text(name, &FeatureProperties::default(), zoom)
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
        let name = shaping::height_offset_property_key(self.uses_shared_height(), component);
        self.number(&name, properties, zoom, 0.0)
    }

    /// Whether a symbol's height includes the terrain elevation below its anchor.
    pub fn height_follows_ground(&self, component: &str) -> bool {
        shaping::height_follows_ground_value(&self.properties, component)
    }

    /// Whether symbols crossing tile boundaries (`[0, 4096]`) are suppressed (`symbol-avoid-edges`).
    pub fn avoid_edges(&self) -> bool {
        shaping::avoid_edges_value(&self.properties)
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
        let transformed = shaping::apply_text_transform(
            text,
            self.text("text-transform", properties, zoom).as_deref(),
        );
        shaping::trim_raw_label_text(&transformed)
    }

    /// Font stack requested by the style.
    pub fn font_stack(&self) -> String {
        shaping::extract_font_stack(&self.properties)
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
        let transformed = shaping::apply_text_transform(
            text,
            self.text("text-transform", properties, zoom).as_deref(),
        );
        Some(shaping::shape_text_sections(transformed, sections))
    }

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
        shaping::trim_shaped_sections(&text, sections)
    }
}

#[cfg(test)]
#[path = "symbol/tests.rs"]
mod tests;

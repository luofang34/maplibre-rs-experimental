//! Typed MapLibre Style Specification v8 layer and property descriptors generated at build
//! time from `maplibre/style-spec-v8.json`.

use std::{fmt, str::FromStr};

use crate::style::expression::{LegacyPropertySpec, PropertyKind};

/// Error returned when parsing an unrecognized style-spec layer, parameter, or property name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSpecIdentifier(pub String);

impl fmt::Display for UnknownSpecIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown style-spec identifier `{}`", self.0)
    }
}

impl std::error::Error for UnknownSpecIdentifier {}

/// Whether a style-spec layer property belongs to `paint` or `layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecPropertyKind {
    /// A layer `paint` property.
    Paint,
    /// A layer `layout` property.
    Layout,
}

impl SpecPropertyKind {
    /// Canonical table scope (`"paint"` or `"layout"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Paint => "paint",
            Self::Layout => "layout",
        }
    }
}

impl fmt::Display for SpecPropertyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Element value type of an `array` style property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecArrayItemType {
    /// Numeric array element (`number`).
    Number,
    /// String array element (`string`).
    String,
    /// Boolean array element (`boolean`).
    Boolean,
    /// Enumerated string array element (`enum`).
    Enum,
    /// Untyped JSON value element (`value`).
    Value,
}

/// Style-spec value type of a `paint` or `layout` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecValueType {
    /// Floating-point scalar (`number`).
    Number,
    /// CSS / RGBA colour (`color`).
    Color,
    /// UTF-8 string (`string`).
    String,
    /// Boolean flag (`boolean`).
    Boolean,
    /// String restricted to [`SpecPropertyMeta::enum_values`] (`enum`).
    Enum,
    /// Homogeneous array with item type and optional fixed length (`array`).
    Array {
        /// Element type of the array.
        item: SpecArrayItemType,
        /// Fixed array length when specified by the style spec.
        length: Option<usize>,
    },
    /// Rich formatted text (`formatted`).
    Formatted,
    /// Sprite or host image name (`resolvedImage`).
    ResolvedImage,
    /// Directional `[top, right, bottom, left]` box padding (`padding`).
    Padding,
    /// Variable-length numeric list (`numberArray`).
    NumberArray,
    /// Variable-length colour list (`colorArray`).
    ColorArray,
    /// Alternating anchor and `[x, y]` offset pairs (`variableAnchorOffsetCollection`).
    VariableAnchorOffsetCollection,
    /// Map projection name or `[from, to, transition]` blend (`projectionDefinition`).
    ProjectionDefinition,
}

/// Input parameter permitted inside expressions for a style-spec property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecExpressionParameter {
    /// Camera zoom (`zoom`).
    Zoom,
    /// Per-feature attributes (`feature`).
    Feature,
    /// Dynamic per-feature state (`feature-state`).
    FeatureState,
    /// Distance fraction along a line (`line-progress`).
    LineProgress,
    /// Kernel density in a heatmap (`heatmap-density`).
    HeatmapDensity,
    /// Radial distance in a sky dome (`sky-radial-progress`).
    SkyRadialProgress,
    /// Scene light measurement (`measure-light`).
    MeasureLight,
    /// Terrain / DEM elevation (`elevation`).
    Elevation,
}

impl SpecExpressionParameter {
    /// Canonical kebab-case parameter name in `style-spec-v8.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zoom => "zoom",
            Self::Feature => "feature",
            Self::FeatureState => "feature-state",
            Self::LineProgress => "line-progress",
            Self::HeatmapDensity => "heatmap-density",
            Self::SkyRadialProgress => "sky-radial-progress",
            Self::MeasureLight => "measure-light",
            Self::Elevation => "elevation",
        }
    }
}

impl fmt::Display for SpecExpressionParameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SpecExpressionParameter {
    type Err = UnknownSpecIdentifier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "zoom" => Ok(Self::Zoom),
            "feature" => Ok(Self::Feature),
            "feature-state" => Ok(Self::FeatureState),
            "line-progress" => Ok(Self::LineProgress),
            "heatmap-density" => Ok(Self::HeatmapDensity),
            "sky-radial-progress" => Ok(Self::SkyRadialProgress),
            "measure-light" => Ok(Self::MeasureLight),
            "elevation" => Ok(Self::Elevation),
            _ => Err(UnknownSpecIdentifier(s.to_owned())),
        }
    }
}

/// Expression support capabilities declared by a style-spec property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpecPropertyExpression {
    /// Whether continuous `interpolate` ramps are supported.
    pub supports_interpolated: bool,
    /// Whether piecewise-constant (`step`, `match`, `case`) expressions are supported.
    pub supports_piecewise_constant: bool,
    /// Evaluation parameters (`zoom`, `feature`, `feature-state`, etc.) allowed in expressions.
    pub parameters: &'static [SpecExpressionParameter],
}

/// Complete build-time metadata for one style-spec `paint` or `layout` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpecPropertyMeta {
    /// Canonical kebab-case property name (e.g. `"fill-color"`).
    pub name: &'static str,
    /// Layer type that owns this property.
    pub layer_type: SpecLayerType,
    /// Whether this is a `paint` or `layout` property.
    pub kind: SpecPropertyKind,
    /// Value type accepted by this property.
    pub value_type: SpecValueType,
    /// Expression capabilities and allowed input parameters.
    pub expression: SpecPropertyExpression,
    /// Allowed string literals when `value_type` is `Enum` or `Array { item: Enum, .. }`.
    pub enum_values: &'static [&'static str],
    /// Whether `<property>-transition` is supported on `paint`.
    pub supports_transition: bool,
    /// Whether `{token}` string templates are lowered for this property.
    pub supports_tokens: bool,
    /// Serialized JSON representation of the specification default, when defined.
    pub default_json: Option<&'static str>,
    /// Specification documentation text.
    pub doc: &'static str,
}

impl SpecPropertyMeta {
    /// Parses [`Self::default_json`] into a `serde_json::Value`, when the spec defines a default.
    pub fn default_value(&self) -> Option<serde_json::Value> {
        self.default_json
            .and_then(|raw| serde_json::from_str(raw).ok())
    }

    /// Converts [`Self::value_type`] and [`Self::enum_values`] into an expression [`PropertyKind`].
    pub fn property_kind(&self) -> PropertyKind {
        let enum_strings = || {
            self.enum_values
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        };
        match self.value_type {
            SpecValueType::Number => PropertyKind::Number,
            SpecValueType::Color => PropertyKind::Color,
            SpecValueType::String => PropertyKind::String,
            SpecValueType::Boolean => PropertyKind::Boolean,
            SpecValueType::Enum => PropertyKind::Enum(enum_strings()),
            SpecValueType::Formatted => PropertyKind::Formatted,
            SpecValueType::ResolvedImage => PropertyKind::ResolvedImage,
            SpecValueType::Padding => PropertyKind::Padding,
            SpecValueType::NumberArray => PropertyKind::NumberArray,
            SpecValueType::ColorArray => PropertyKind::ColorArray,
            SpecValueType::VariableAnchorOffsetCollection => {
                PropertyKind::VariableAnchorOffsetCollection
            }
            SpecValueType::ProjectionDefinition => PropertyKind::ProjectionDefinition,
            SpecValueType::Array { item, length } => PropertyKind::Array {
                item: Box::new(match item {
                    SpecArrayItemType::Number => PropertyKind::Number,
                    SpecArrayItemType::String => PropertyKind::String,
                    SpecArrayItemType::Boolean => PropertyKind::Boolean,
                    SpecArrayItemType::Enum => PropertyKind::Enum(enum_strings()),
                    SpecArrayItemType::Value => PropertyKind::Value,
                }),
                length,
            },
        }
    }

    /// Builds the [`LegacyPropertySpec`] used to parse constants, `{token}` templates,
    /// legacy functions, and expressions for this property.
    pub fn legacy_property_spec(&self) -> LegacyPropertySpec {
        LegacyPropertySpec {
            kind: self.property_kind(),
            interpolated: self.expression.supports_interpolated,
            default: self.default_value(),
            tokens: self.supports_tokens,
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/style_spec_v8.rs"));

impl SpecLayerType {
    /// Whether `maplibre`'s layer pipeline renders layers of this `type` directly.
    pub const fn is_rendered_layer(self) -> bool {
        !matches!(self, Self::Sky | Self::Custom)
    }
}

/// Looks up the static descriptor for a `paint` property on `layer_type`.
pub fn lookup_paint_property(layer_type: &str, name: &str) -> Option<&'static SpecPropertyMeta> {
    let layer = SpecLayerType::from_str(layer_type).ok()?;
    match layer {
        SpecLayerType::Fill => FillPaintProperty::from_str(name)
            .ok()
            .map(FillPaintProperty::metadata),
        SpecLayerType::Line => LinePaintProperty::from_str(name)
            .ok()
            .map(LinePaintProperty::metadata),
        SpecLayerType::Symbol => SymbolPaintProperty::from_str(name)
            .ok()
            .map(SymbolPaintProperty::metadata),
        SpecLayerType::Circle => CirclePaintProperty::from_str(name)
            .ok()
            .map(CirclePaintProperty::metadata),
        SpecLayerType::Heatmap => HeatmapPaintProperty::from_str(name)
            .ok()
            .map(HeatmapPaintProperty::metadata),
        SpecLayerType::FillExtrusion => FillExtrusionPaintProperty::from_str(name)
            .ok()
            .map(FillExtrusionPaintProperty::metadata),
        SpecLayerType::Raster => RasterPaintProperty::from_str(name)
            .ok()
            .map(RasterPaintProperty::metadata),
        SpecLayerType::Hillshade => HillshadePaintProperty::from_str(name)
            .ok()
            .map(HillshadePaintProperty::metadata),
        SpecLayerType::Background => BackgroundPaintProperty::from_str(name)
            .ok()
            .map(BackgroundPaintProperty::metadata),
        SpecLayerType::Sky => SkyPaintProperty::from_str(name)
            .ok()
            .map(SkyPaintProperty::metadata),
        SpecLayerType::ColorRelief => ColorReliefPaintProperty::from_str(name)
            .ok()
            .map(ColorReliefPaintProperty::metadata),
        SpecLayerType::Custom => None,
    }
}

/// Looks up the static descriptor for a `layout` property on `layer_type`.
pub fn lookup_layout_property(layer_type: &str, name: &str) -> Option<&'static SpecPropertyMeta> {
    let layer = SpecLayerType::from_str(layer_type).ok()?;
    match layer {
        SpecLayerType::Fill => FillLayoutProperty::from_str(name)
            .ok()
            .map(FillLayoutProperty::metadata),
        SpecLayerType::Line => LineLayoutProperty::from_str(name)
            .ok()
            .map(LineLayoutProperty::metadata),
        SpecLayerType::Symbol => SymbolLayoutProperty::from_str(name)
            .ok()
            .map(SymbolLayoutProperty::metadata),
        SpecLayerType::Circle => CircleLayoutProperty::from_str(name)
            .ok()
            .map(CircleLayoutProperty::metadata),
        SpecLayerType::Heatmap => HeatmapLayoutProperty::from_str(name)
            .ok()
            .map(HeatmapLayoutProperty::metadata),
        SpecLayerType::FillExtrusion => FillExtrusionLayoutProperty::from_str(name)
            .ok()
            .map(FillExtrusionLayoutProperty::metadata),
        SpecLayerType::Raster => RasterLayoutProperty::from_str(name)
            .ok()
            .map(RasterLayoutProperty::metadata),
        SpecLayerType::Hillshade => HillshadeLayoutProperty::from_str(name)
            .ok()
            .map(HillshadeLayoutProperty::metadata),
        SpecLayerType::Background => BackgroundLayoutProperty::from_str(name)
            .ok()
            .map(BackgroundLayoutProperty::metadata),
        SpecLayerType::Sky => SkyLayoutProperty::from_str(name)
            .ok()
            .map(SkyLayoutProperty::metadata),
        SpecLayerType::ColorRelief => ColorReliefLayoutProperty::from_str(name)
            .ok()
            .map(ColorReliefLayoutProperty::metadata),
        SpecLayerType::Custom => None,
    }
}

#[cfg(test)]
mod tests;

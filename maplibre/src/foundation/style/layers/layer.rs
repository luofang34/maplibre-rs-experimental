//! Layer identity, visibility and typed paint properties.

use std::hash::{Hash, Hasher};

use cint::{Alpha, EncodedSrgb};
use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};

pub use crate::style::property::{
    FormattedText, ImageName, PatternName, PropertyValue, StyleProperty, TextField,
};
use crate::style::{
    circle::CirclePaint,
    heatmap::HeatmapPaint,
    hillshade::{ColorReliefPaint, HillshadePaint},
};

pub(crate) mod classify;
mod datum;
mod fill_outline;
mod paint;
mod paint_from_value;
mod referenced_keys;
mod serialization;
mod symbol_serde;
pub(crate) mod uniform_paint;
pub use datum::{ElevationDatum, ExtrusionPart};
pub use paint::{
    BackgroundPaint, CustomPaint, FillExtrusionPaint, FillPaint, LinePaint, RasterPaint,
    RasterResampling, SymbolPaint, TranslateAnchor,
};
pub(crate) use referenced_keys::{collect_layer_referenced_keys, symbol_runtime_referenced_keys};
pub use serialization::LayerProperties;

/// Typed properties selected by a style layer's rendering type.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "paint")]
pub enum LayerPaint {
    /// Background color beneath source-backed layers.
    #[serde(rename = "background")]
    Background(BackgroundPaint),
    /// Stroked paths with width, opacity, translation and dash properties.
    #[serde(rename = "line")]
    Line(LinePaint),
    /// Filled polygon color, opacity and translation.
    #[serde(rename = "fill")]
    Fill(FillPaint),
    /// Polygons raised to a height, with lit walls and roof.
    #[serde(rename = "fill-extrusion")]
    FillExtrusion(FillExtrusionPaint),
    /// Image sampling and adjustment properties retained from the style.
    #[serde(rename = "raster")]
    Raster(RasterPaint),
    /// Elevation-dependent illumination and shadow colors.
    #[serde(rename = "hillshade")]
    Hillshade(HillshadePaint),
    /// Color ramp evaluated against decoded elevation.
    #[serde(rename = "color-relief")]
    ColorRelief(ColorReliefPaint),
    /// Text and icon appearance and layout properties.
    #[serde(rename = "symbol")]
    Symbol(SymbolPaint),
    /// Point radius, fill, stroke and alignment properties.
    #[serde(rename = "circle")]
    Circle(CirclePaint),
    /// Point density radius, weight, intensity, opacity and colour ramp.
    #[serde(rename = "heatmap")]
    Heatmap(HeatmapPaint),
    /// Host-provided GPU custom style layer (`"type": "custom"`).
    #[serde(rename = "custom")]
    Custom(CustomPaint),
}

impl LayerPaint {
    /// The opacity property multiplied into the layer's colour, when the layer type has one.
    pub fn opacity(&self) -> Option<StyleProperty<f32>> {
        match self {
            LayerPaint::Fill(paint) => paint.fill_opacity.clone(),
            LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_translucent == Some(true) => {
                paint.fill_extrusion_opacity.clone()
            }
            LayerPaint::Line(paint) => paint.line_opacity.clone(),
            LayerPaint::Circle(paint) => paint.circle_opacity.clone(),
            LayerPaint::ColorRelief(paint) => paint.color_relief_opacity.clone(),
            LayerPaint::Background(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Raster(_)
            | LayerPaint::Hillshade(_)
            | LayerPaint::Symbol(_)
            | LayerPaint::Heatmap(_)
            | LayerPaint::Custom(_) => None,
        }
    }

    /// Returns a constant base color in encoded sRGB with straight alpha.
    /// Expressions, absent colors and paint families without one base color return `None`.
    /// Layer opacity is not multiplied into this value.
    pub fn get_color(&self) -> Option<Alpha<EncodedSrgb<f32>>> {
        let property = match self {
            Self::Background(paint) => &paint.background_color,
            Self::Line(paint) => &paint.line_color,
            Self::Fill(paint) => &paint.fill_color,
            Self::Circle(paint) => &paint.circle_color,
            Self::FillExtrusion(paint) => &paint.fill_extrusion_color,
            Self::Raster(_)
            | Self::Hillshade(_)
            | Self::ColorRelief(_)
            | Self::Symbol(_)
            | Self::Heatmap(_)
            | Self::Custom(_) => return None,
        };
        match property.as_ref()? {
            StyleProperty::Constant(color) => Some((*color).into()),
            _ => None,
        }
    }

    /// Evaluates a feature-constant base color in encoded sRGB at `zoom`.
    /// Data-driven color expressions and paint families without one base color return `None`.
    pub fn color_at(&self, zoom: f64) -> Option<Alpha<EncodedSrgb<f32>>> {
        let property = match self {
            Self::Background(paint) => &paint.background_color,
            Self::Line(paint) => &paint.line_color,
            Self::Circle(paint) => &paint.circle_color,
            Self::Fill(paint) => &paint.fill_color,
            Self::FillExtrusion(paint) => &paint.fill_extrusion_color,
            Self::Raster(_)
            | Self::Hillshade(_)
            | Self::ColorRelief(_)
            | Self::Symbol(_)
            | Self::Heatmap(_)
            | Self::Custom(_) => return None,
        };
        let color = property.as_ref()?.evaluate_at_zoom(zoom)?;
        Some(color.into())
    }
}

/// Whether a layer is drawn at all, the `layout.visibility` property.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum LayerVisibility {
    /// The layer is drawn.
    #[default]
    #[serde(rename = "visible")]
    Visible,
    /// The layer is neither laid out nor drawn.
    #[serde(rename = "none")]
    None,
}

/// Bitflag: layer paint or symbol layout reads `["id"]` / `["geometry-type"]`.
pub(crate) const FLAG_READS_IDENTITY: u8 = 1;
/// Bitflag: layer filter, paint or layout reads `["within", ...]` / `["distance", ...]`.
pub(crate) const FLAG_READS_GEOMETRY: u8 = 2;
/// Bitflag: layer paint reads `["feature-state", ...]`.
pub(crate) const FLAG_READS_FEATURE_STATE: u8 = 4;
/// Bitflag: layer geometry-affecting properties read `["feature-state", ...]`.
pub(crate) const FLAG_GEOM_READS_FEATURE_STATE: u8 = 8;

/// One entry in the style's painter order. Equality and hashing use only the layer ID.
/// Zoom bounds control display visibility independently of source tile zoom limits.
#[derive(Debug, Clone)]
pub struct StyleLayer {
    /// Zero-based painter order assigned when the containing style is deserialized.
    pub index: u32,
    /// Document-local identity used for equality, hashing and matching processed geometry.
    pub id: String,
    /// Rendering type from JSON, used to select paint parsing and the corresponding pipeline.
    pub type_: String,
    /// Feature-selection expression retained as JSON for filter validation and evaluation.
    pub filter: Option<serde_json::Value>,
    /// Exclusive upper display zoom; fractional values are preserved. `None` sets no upper bound.
    pub maxzoom: Option<f64>,
    /// Inclusive lower display zoom; fractional values are preserved. `None` sets no lower bound.
    pub minzoom: Option<f64>,
    /// Application JSON properties retained across worker serialization.
    /// Reserved `maplibre-rs:terrain-structure` keys opt into terrain-relative road profiles.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Typed rendering properties; absence lets the relevant rendering path choose defaults.
    pub paint: Option<LayerPaint>,
    /// Key of a source in the containing style, or `None` for source-independent or host-supplied data.
    pub source: Option<String>,
    /// Layer name inside a vector tile, distinct from the style layer ID.
    pub source_layer: Option<String>,
    /// Whether the layer is drawn at all.
    pub visibility: LayerVisibility,
    /// Per-property `<prop>-transition` timing declarations carried in `paint`.
    pub transitions: rustc_hash::FxHashMap<String, crate::style::property::TransitionSpec>,
    /// Properties without a typed representation, retained for diagnostics and round trips.
    pub unrecognized: LayerProperties,
    /// Precomputed input bitmask from deserialization, avoiding `Serialize` round-trips.
    #[doc(hidden)]
    pub input_flags: Option<u8>,
}

impl StyleLayer {
    /// Interned 32-bit [`StyleLayerId`](crate::style::StyleLayerId) for zero-allocation phase items
    /// and GPU lookup tables.
    #[inline]
    pub fn layer_id(&self) -> crate::style::StyleLayerId {
        crate::style::StyleLayerId::intern(&self.id)
    }

    /// Interned 32-bit [`SourceLayerId`](crate::style::SourceLayerId) when `source_layer` is set.
    #[inline]
    pub fn source_layer_id(&self) -> Option<crate::style::SourceLayerId> {
        self.source_layer
            .as_deref()
            .map(crate::style::SourceLayerId::intern)
    }

    /// Whether the layer is switched off by its `layout.visibility`, at every zoom.
    pub fn is_hidden(&self) -> bool {
        self.visibility == LayerVisibility::None
    }

    /// Returns whether the layer is drawn at a continuous zoom: it is not hidden and the zoom
    /// lies in its range, using the style-spec rule that `minzoom` is inclusive and `maxzoom`
    /// exclusive. A `minzoom` of `0` (the lowest tile zoom) stays visible at negative globe zooms,
    /// matching MapLibre GL JS (`this.minzoom && zoom < this.minzoom`).
    pub fn is_visible_at(&self, zoom: f64) -> bool {
        !self.is_hidden()
            && self
                .minzoom
                .is_none_or(|minzoom| minzoom <= 0.0 || zoom >= minzoom)
            && self.maxzoom.is_none_or(|maxzoom| zoom < maxzoom)
    }

    /// Returns whether this layer can appear at any continuous zoom `[tile_zoom, tile_zoom + 1)`
    /// covered by a tile built for `tile_zoom` (`minzoom.floor() <= tile_zoom < maxzoom.ceil()`).
    pub fn is_visible_at_tile_zoom(&self, tile_zoom: u8) -> bool {
        !self.is_hidden()
            && self
                .minzoom
                .is_none_or(|minzoom| minzoom <= 0.0 || f64::from(tile_zoom) >= minzoom.floor())
            && self
                .maxzoom
                .is_none_or(|maxzoom| f64::from(tile_zoom) < maxzoom.ceil())
    }

    /// Whether a vector worker request for `coords` at `overscaled_zoom` needs to build this layer.
    pub(crate) fn needed_for_tile(
        &self,
        terrain: bool,
        coords: crate::coords::WorldTileCoords,
        overscaled_zoom: u8,
    ) -> bool {
        if terrain && u8::from(coords.z) == 0 {
            return !self.is_hidden() && self.maxzoom.is_none_or(|maxzoom| 0.0 < maxzoom.ceil());
        }
        self.is_visible_at_tile_zoom(overscaled_zoom.max(u8::from(coords.z)))
    }

    /// Whether both layers carry identical typed, transition and unrecognized paint properties.
    pub(crate) fn same_paint(&self, other: &Self) -> bool {
        serde_json::to_value(&self.paint).ok() == serde_json::to_value(&other.paint).ok()
            && self.transitions == other.transitions
            && self.unrecognized.paint == other.unrecognized.paint
    }

    /// Whether both layers share identical GPU-side draw order, visibility, zoom bounds, metadata
    /// and paint properties.
    pub(crate) fn same_gpu_definition(&self, other: &Self) -> bool {
        self.index == other.index
            && self.type_ == other.type_
            && self.source == other.source
            && self.source_layer == other.source_layer
            && self.visibility == other.visibility
            && self.minzoom == other.minzoom
            && self.maxzoom == other.maxzoom
            && self.metadata == other.metadata
            && self.same_paint(other)
    }
}

impl Eq for StyleLayer {}
impl PartialEq for StyleLayer {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id)
    }
}

impl Hash for StyleLayer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl Default for StyleLayer {
    fn default() -> Self {
        Self {
            index: 0,
            id: "id".to_string(),
            type_: "background".to_string(),
            filter: None,
            maxzoom: None,
            minzoom: None,
            metadata: None,
            paint: None,
            source: None,
            source_layer: Some("does not exist".to_string()),
            visibility: LayerVisibility::Visible,
            transitions: Default::default(),
            unrecognized: Default::default(),
            input_flags: None,
        }
    }
}

/// Layer types drawn from image tiles: plain raster imagery and the DEM-shaded layers.
pub const RASTER_LAYER_TYPES: [&str; 3] = ["raster", "hillshade", "color-relief"];

/// Whether a layer's content lives in per-tile buffers built from vector tiles.
pub(crate) fn from_vector_tiles(layer: &StyleLayer) -> bool {
    layer.type_ != "background"
        && layer.type_ != "custom"
        && !RASTER_LAYER_TYPES.contains(&layer.type_.as_str())
}

/// Whether `layer`'s paint or layout reads the feature's id or geometry type.
pub(crate) fn layer_reads_identity(layer: &StyleLayer) -> bool {
    if let Some(flags) = layer.input_flags {
        return (flags & FLAG_READS_IDENTITY) != 0;
    }
    #[cfg(any(test, not(target_arch = "wasm32")))]
    {
        serde_json::to_value(&layer.paint)
            .is_ok_and(|paint| crate::style::expression::reads_identity(&paint))
    }
    #[cfg(not(any(test, not(target_arch = "wasm32"))))]
    {
        false
    }
}

/// Whether `layer` has an expression that reads the feature's geometry.
pub(crate) fn layer_reads_geometry(layer: &StyleLayer) -> bool {
    if let Some(flags) = layer.input_flags {
        return (flags & FLAG_READS_GEOMETRY) != 0;
    }
    #[cfg(any(test, not(target_arch = "wasm32")))]
    {
        serde_json::to_string(layer)
            .is_ok_and(|text| text.contains("\"within\"") || text.contains("\"distance\""))
    }
    #[cfg(not(any(test, not(target_arch = "wasm32"))))]
    {
        false
    }
}

/// A layer's `*-translate` at `zoom` in screen pixels along the map's axes: a translation
/// anchored to the viewport turns with the view's `bearing`, in radians.
pub fn layer_translate_pixels(paint: Option<&LayerPaint>, zoom: f64, bearing: f64) -> [f64; 2] {
    use crate::style::translation::translation_at;
    let (translate, anchor) = match paint {
        Some(LayerPaint::Fill(paint)) => (
            translation_at(paint.fill_translate.as_ref(), zoom),
            paint.fill_translate_anchor,
        ),
        Some(LayerPaint::Line(paint)) => (
            translation_at(paint.line_translate.as_ref(), zoom),
            paint.line_translate_anchor,
        ),
        Some(LayerPaint::Circle(paint)) => (
            translation_at(paint.circle_translate.as_ref(), zoom),
            paint.circle_translate_anchor,
        ),
        Some(LayerPaint::FillExtrusion(paint)) => (
            translation_at(paint.fill_extrusion_translate.as_ref(), zoom),
            paint.fill_extrusion_translate_anchor,
        ),
        _ => return [0.0; 2],
    };
    let [x, y] = translate.map(f64::from);
    if anchor == TranslateAnchor::Viewport {
        let (sin, cos) = bearing.sin_cos();
        [x * cos - y * sin, x * sin + y * cos]
    } else {
        [x, y]
    }
}

#[cfg(test)]
mod tests;

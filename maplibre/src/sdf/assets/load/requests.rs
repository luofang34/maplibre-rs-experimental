//! The glyphs and images the features of one tile ask for under a source's symbol layers.

use std::cell::RefCell;

use geozero::mvt::Message;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    style::{
        expression::ImageSet,
        layer::{LayerPaint, StyleLayer},
    },
    vector::fill_feature_properties,
};

/// Characters wanted from each font stack.
pub(super) type GlyphRequests = HashMap<String, HashSet<u32>>;

/// Glyph ranges, plain label strings (for OpenType shaping), and sprite names requested by one
/// tile's visible symbol features.
#[derive(Default)]
pub(crate) struct SymbolRequests {
    pub(super) fonts: GlyphRequests,
    pub(super) labels: HashMap<String, HashSet<String>>,
    pub(super) icons: HashSet<String>,
}

/// Answers that no image exists and remembers every name asked about, so evaluating an
/// expression against it walks each `image` it may reach and records that image's name,
/// computed names and the fallbacks of a `coalesce` included.
#[derive(Debug, Default)]
struct NamesAsked(RefCell<Vec<String>>);

impl ImageSet for NamesAsked {
    fn contains_image(&self, name: &str) -> bool {
        self.0.borrow_mut().push(name.to_owned());
        false
    }
}

fn needs_label_shaping(text: &str) -> bool {
    text.chars().any(|ch| {
        crate::font::is_complex_shaping_codepoint(ch)
            || matches!(u32::from(ch), 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
    })
}

/// Adds the characters of a feature's label to the fonts that draw them, as the label reads
/// with `images` held.
fn glyphs(
    fonts: &mut GlyphRequests,
    labels: &mut HashMap<String, HashSet<String>>,
    paint: &crate::style::layer::SymbolPaint,
    (properties, zoom): (&crate::style::expression::FeatureProperties, f64),
    images: &dyn ImageSet,
    has_registered_fonts: bool,
) {
    let Some(text) = paint.label_among(properties, zoom, Some(images)) else {
        return;
    };
    let stack = paint.font_stack();
    if has_registered_fonts || needs_label_shaping(&text) {
        if let Some(raw) = paint.raw_label_among(properties, zoom, Some(images)) {
            labels.entry(stack.clone()).or_default().insert(raw);
        }
    }
    fonts.entry(stack).or_default().extend(
        text.chars()
            .filter(|c| *c != crate::style::expression::FORMAT_IMAGE)
            .map(|c| c as u32),
    );
    // A section with a font of its own needs that font's glyphs for its characters.
    let mut characters = text.chars();
    for section in paint.label_sections_among(properties, zoom, Some(images)) {
        let own: Vec<char> = characters.by_ref().take(section.length).collect();
        if let Some(font) = &section.font {
            fonts
                .entry(font.clone())
                .or_default()
                .extend(own.into_iter().map(|c| c as u32));
        }
    }
}

#[cfg(test)]
pub(super) fn requests<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> (GlyphRequests, HashSet<String>) {
    let req = symbol_requests(layers, data, zoom, style);
    (req.fonts, req.icons)
}

pub(super) fn symbol_requests<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> SymbolRequests {
    #[cfg(feature = "mlt")]
    if !crate::vector::looks_like_mvt(data) && crate::vector::mlt::is_valid_mlt(data) {
        return symbol_requests_from_mlt(layers, data, zoom, style);
    }
    let Ok(tile) = geozero::mvt::Tile::decode(data) else {
        return SymbolRequests::default();
    };
    symbol_requests_from_mvt(layers, &tile, zoom, style)
}

pub(crate) fn symbol_requests_from_tile<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    tile: &crate::vector::DecodedVectorTile,
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> SymbolRequests {
    match tile {
        crate::vector::DecodedVectorTile::Mvt(mvt) => {
            symbol_requests_from_mvt(layers, mvt, zoom, style)
        }
        #[cfg(feature = "mlt")]
        crate::vector::DecodedVectorTile::Mlt(bytes) => {
            symbol_requests_from_mlt(layers, bytes, zoom, style)
        }
    }
}

struct CompiledSymbolLayer<'l, 's> {
    paint: &'l crate::style::layer::SymbolPaint,
    filter: Option<crate::style::filter::Filter>,
    promote_prop: Option<&'s str>,
    source_state: Option<&'s crate::style::feature_state::SourceFeatureState>,
}

fn compile_symbol_layers<'l, 's>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    style: Option<&'s crate::style::Style>,
) -> HashMap<&'l str, Vec<CompiledSymbolLayer<'l, 's>>> {
    let mut by_source_layer: HashMap<&str, Vec<CompiledSymbolLayer<'_, '_>>> = HashMap::default();
    for layer in layers {
        if layer.is_hidden() {
            continue;
        }
        let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
            continue;
        };
        let Some(source_name) = layer.source_layer.as_deref() else {
            continue;
        };
        let filter = match layer
            .filter
            .as_ref()
            .map(crate::style::filter::Filter::parse)
        {
            Some(Ok(filter)) => Some(filter),
            Some(Err(_)) => continue,
            None => None,
        };
        let (promote_prop, source_state) = match (style, layer.source.as_deref()) {
            (Some(style), Some(src_name)) => (
                style
                    .source_promote_id(src_name)
                    .and_then(|p| p.property_for(Some(source_name))),
                style.feature_state.source(src_name),
            ),
            _ => (None, None),
        };
        by_source_layer
            .entry(source_name)
            .or_default()
            .push(CompiledSymbolLayer {
                paint,
                filter,
                promote_prop,
                source_state,
            });
    }
    by_source_layer
}

#[allow(clippy::too_many_arguments)]
fn record_feature_symbol_requests(
    out: &mut SymbolRequests,
    compiled_layers: &[CompiledSymbolLayer<'_, '_>],
    source_name: &str,
    raw_id: Option<u64>,
    geometry_type: crate::style::filter::GeometryType,
    base_properties: &crate::style::expression::FeatureProperties,
    zoom: f64,
    has_registered_fonts: bool,
) {
    let feature_id = raw_id.map(|id| crate::style::expression::Value::Number(id as f64));
    let mut state_scratch: Option<crate::style::expression::FeatureProperties> = None;

    for compiled in compiled_layers {
        let props = if compiled.source_state.is_some() || compiled.promote_prop.is_some() {
            let scratch = state_scratch.insert(base_properties.clone());
            crate::style::feature_state::sync_properties_feature_state(
                scratch,
                raw_id,
                compiled.promote_prop,
                compiled.source_state,
                Some(source_name),
            );
            &*scratch
        } else {
            base_properties
        };
        if let Some(filter) = &compiled.filter {
            if !filter.evaluate(&crate::style::filter::FeatureContext {
                properties: props,
                geometry_type,
                id: feature_id.clone(),
                zoom,
            }) {
                continue;
            }
        }
        let asked = NamesAsked::default();
        glyphs(
            &mut out.fonts,
            &mut out.labels,
            compiled.paint,
            (props, zoom),
            &asked,
            has_registered_fonts,
        );
        if let Some(icon) = compiled
            .paint
            .text_among_images("icon-image", props, zoom, &asked)
            .filter(|icon| !icon.is_empty())
        {
            out.icons.insert(icon);
        }
        out.icons.extend(
            compiled
                .paint
                .label_sections_among(props, zoom, Some(&asked))
                .into_iter()
                .filter_map(|section| section.image),
        );
        out.icons.extend(asked.0.into_inner());
    }
}

fn symbol_requests_from_mvt<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    tile: &geozero::mvt::Tile,
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> SymbolRequests {
    const MAX_TAG_MEMO: usize = 128;
    let by_source_layer = compile_symbol_layers(layers, style);
    let mut out = SymbolRequests::default();
    if by_source_layer.is_empty() {
        return out;
    }
    let has_registered_fonts = style.is_some_and(|s| s.font_faces.is_some())
        || crate::font::global_font_system().has_registered_any();

    let empty_props = crate::style::expression::empty_feature_properties();
    for source in &tile.layers {
        let Some(compiled_layers) = by_source_layer.get(source.name.as_str()) else {
            continue;
        };
        let mut tag_memo: rustc_hash::FxHashMap<
            &[u32],
            crate::style::expression::FeatureProperties,
        > = rustc_hash::FxHashMap::default();
        let mut scratch = crate::style::expression::FeatureProperties::default();
        for feature in &source.features {
            let base_properties: &crate::style::expression::FeatureProperties =
                if feature.tags.is_empty() {
                    empty_props
                } else if let Some(existing) = tag_memo.get(feature.tags.as_slice()) {
                    existing
                } else if tag_memo.len() < MAX_TAG_MEMO {
                    let mut props = crate::style::expression::FeatureProperties::default();
                    fill_feature_properties(source, feature, &mut props);
                    tag_memo.entry(feature.tags.as_slice()).or_insert(props)
                } else {
                    fill_feature_properties(source, feature, &mut scratch);
                    &scratch
                };
            let geometry_type =
                crate::style::filter::GeometryType::from_mvt(feature.r#type.unwrap_or_default());
            record_feature_symbol_requests(
                &mut out,
                compiled_layers,
                &source.name,
                feature.id,
                geometry_type,
                base_properties,
                zoom,
                has_registered_fonts,
            );
        }
    }
    out
}

#[cfg(feature = "mlt")]
fn symbol_requests_from_mlt<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> SymbolRequests {
    let by_source_layer = compile_symbol_layers(layers, style);
    let mut out = SymbolRequests::default();
    if by_source_layer.is_empty() {
        return out;
    }
    let has_registered_fonts = style.is_some_and(|s| s.font_faces.is_some())
        || crate::font::global_font_system().has_registered_any();
    crate::vector::mlt::visit_mlt_features(
        data,
        |name| by_source_layer.contains_key(name),
        |source_name, raw_id, geometry_type, base_properties| {
            if let Some(compiled_layers) = by_source_layer.get(source_name) {
                record_feature_symbol_requests(
                    &mut out,
                    compiled_layers,
                    source_name,
                    raw_id,
                    geometry_type,
                    base_properties,
                    zoom,
                    has_registered_fonts,
                );
            }
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{
        expression::{FeatureProperties, Value},
        layer::{StyleProperty, SymbolPaint},
    };

    #[test]
    fn skips_label_shaping_strings_for_latin_cjk_unless_fonts_registered_or_complex() {
        let paint = SymbolPaint {
            text_field: Some(StyleProperty::parse(&serde_json::json!("{name}"))),
            ..Default::default()
        };
        let asked = NamesAsked::default();

        let mut latin_cjk = FeatureProperties::default();
        latin_cjk.insert("name".into(), Value::String("Tokyo 東京".into()));
        let mut fonts = GlyphRequests::default();
        let mut labels = HashMap::default();
        glyphs(
            &mut fonts,
            &mut labels,
            &paint,
            (&latin_cjk, 14.0),
            &asked,
            false,
        );
        assert!(labels.is_empty());
        assert!(!fonts.is_empty());

        let mut arabic = FeatureProperties::default();
        arabic.insert("name".into(), Value::String("مرحبا".into()));
        glyphs(
            &mut fonts,
            &mut labels,
            &paint,
            (&arabic, 14.0),
            &asked,
            false,
        );
        assert!(labels
            .get(&paint.font_stack())
            .is_some_and(|set| set.contains("مرحبا")));

        let mut with_registered = HashMap::default();
        glyphs(
            &mut fonts,
            &mut with_registered,
            &paint,
            (&latin_cjk, 14.0),
            &asked,
            true,
        );
        assert!(with_registered
            .get(&paint.font_stack())
            .is_some_and(|set| set.contains("Tokyo 東京")));
    }
}

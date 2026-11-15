//! The glyphs and images the features of one tile ask for under a source's symbol layers.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
};

use geozero::mvt::Message;

use crate::{
    style::{
        expression::ImageSet,
        layer::{LayerPaint, StyleLayer},
    },
    vector::feature_properties,
};

/// Characters wanted from each font stack.
pub(super) type GlyphRequests = HashMap<String, BTreeSet<u32>>;

/// Glyph ranges, plain label strings (for OpenType shaping), and sprite names requested by one
/// tile's visible symbol features.
#[derive(Default)]
pub(super) struct SymbolRequests {
    pub(super) fonts: GlyphRequests,
    pub(super) labels: HashMap<String, BTreeSet<String>>,
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

/// Adds the characters of a feature's label to the fonts that draw them, as the label reads
/// with `images` held.
fn glyphs(
    fonts: &mut GlyphRequests,
    labels: &mut HashMap<String, BTreeSet<String>>,
    paint: &crate::style::layer::SymbolPaint,
    (properties, zoom): (&crate::style::expression::FeatureProperties, f64),
    images: &dyn ImageSet,
) {
    let Some(text) = paint.label_among(properties, zoom, Some(images)) else {
        return;
    };
    let stack = paint.font_stack();
    if let Some(raw) = paint.raw_label_among(properties, zoom, Some(images)) {
        labels.entry(stack.clone()).or_default().insert(raw);
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
    let Ok(tile) = geozero::mvt::Tile::decode(data) else {
        return SymbolRequests::default();
    };
    symbol_requests_from_tile(layers, &tile, zoom, style)
}

pub(super) fn symbol_requests_from_tile<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    tile: &geozero::mvt::Tile,
    zoom: f64,
    style: Option<&crate::style::Style>,
) -> SymbolRequests {
    struct CompiledSymbolLayer<'a> {
        paint: &'a crate::style::layer::SymbolPaint,
        filter: Option<crate::style::filter::Filter>,
        promote_prop: Option<&'a str>,
        source_state: Option<&'a crate::style::feature_state::SourceFeatureState>,
    }

    let mut by_source_layer: HashMap<&str, Vec<CompiledSymbolLayer<'_>>> = HashMap::new();
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

    let mut fonts = GlyphRequests::new();
    let mut labels: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut icons = HashSet::new();
    if by_source_layer.is_empty() {
        return SymbolRequests {
            fonts,
            labels,
            icons,
        };
    }

    let empty_props = crate::style::expression::empty_feature_properties();
    for source in &tile.layers {
        let Some(compiled_layers) = by_source_layer.get(source.name.as_str()) else {
            continue;
        };
        let mut tag_memo: rustc_hash::FxHashMap<
            &[u32],
            crate::style::expression::FeatureProperties,
        > = rustc_hash::FxHashMap::default();
        for feature in &source.features {
            let base_properties: &crate::style::expression::FeatureProperties =
                if feature.tags.is_empty() {
                    empty_props
                } else {
                    tag_memo
                        .entry(feature.tags.as_slice())
                        .or_insert_with(|| feature_properties(source, feature))
                };
            let geometry_type =
                crate::style::filter::GeometryType::from_mvt(feature.r#type.unwrap_or_default());
            let feature_id = feature
                .id
                .map(|id| crate::style::expression::Value::Number(id as f64));
            let mut state_scratch: Option<crate::style::expression::FeatureProperties> = None;

            for compiled in compiled_layers {
                let props = if compiled.source_state.is_some() || compiled.promote_prop.is_some() {
                    let scratch = state_scratch.insert(base_properties.clone());
                    crate::style::feature_state::sync_properties_feature_state(
                        scratch,
                        feature.id,
                        compiled.promote_prop,
                        compiled.source_state,
                        Some(&source.name),
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
                // Every image the label or icon can reach is asked about, so each is requested.
                let asked = NamesAsked::default();
                glyphs(
                    &mut fonts,
                    &mut labels,
                    compiled.paint,
                    (props, zoom),
                    &asked,
                );
                if let Some(icon) = compiled
                    .paint
                    .text_among_images("icon-image", props, zoom, &asked)
                    .filter(|icon| !icon.is_empty())
                {
                    icons.insert(icon);
                }
                icons.extend(
                    compiled
                        .paint
                        .label_sections_among(props, zoom, Some(&asked))
                        .into_iter()
                        .filter_map(|section| section.image),
                );
                icons.extend(asked.0.into_inner());
            }
        }
    }
    SymbolRequests {
        fonts,
        labels,
        icons,
    }
}

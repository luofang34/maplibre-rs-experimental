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

/// Adds the characters of a feature's label to the fonts that draw them.
fn glyphs(
    fonts: &mut GlyphRequests,
    paint: &crate::style::layer::SymbolPaint,
    properties: &crate::style::expression::FeatureProperties,
    zoom: f64,
) {
    let Some(text) = paint.label(properties, zoom) else {
        return;
    };
    fonts.entry(paint.font_stack()).or_default().extend(
        text.chars()
            .filter(|c| *c != crate::style::expression::FORMAT_IMAGE)
            .map(|c| c as u32),
    );
    // A section with a font of its own needs that font's glyphs for its characters.
    let mut characters = text.chars();
    for section in paint.label_sections(properties, zoom) {
        let own: Vec<char> = characters.by_ref().take(section.length).collect();
        if let Some(font) = &section.font {
            fonts
                .entry(font.clone())
                .or_default()
                .extend(own.into_iter().map(|c| c as u32));
        }
    }
}

pub(super) fn requests<'l>(
    layers: impl IntoIterator<Item = &'l StyleLayer>,
    data: &[u8],
    zoom: f64,
) -> (GlyphRequests, HashSet<String>) {
    let mut fonts = GlyphRequests::new();
    let mut icons = HashSet::new();
    let Ok(tile) = geozero::mvt::Tile::decode(data) else {
        return (fonts, icons);
    };
    for layer in layers {
        if layer.is_hidden() {
            continue;
        }
        let Some(LayerPaint::Symbol(paint)) = &layer.paint else {
            continue;
        };
        let Some(source) = tile
            .layers
            .iter()
            .find(|source| Some(&source.name) == layer.source_layer.as_ref())
        else {
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
        for feature in &source.features {
            let properties = feature_properties(source, feature);
            if let Some(filter) = &filter {
                if !filter.evaluate(&crate::style::filter::FeatureContext {
                    properties: &properties,
                    geometry_type: crate::style::filter::GeometryType::from_mvt(
                        feature.r#type.unwrap_or_default(),
                    ),
                    id: feature
                        .id
                        .map(|id| crate::style::expression::Value::Number(id as f64)),
                    zoom,
                }) {
                    continue;
                }
            }
            glyphs(&mut fonts, paint, &properties, zoom);
            let asked = NamesAsked::default();
            if let Some(icon) = paint
                .text_among_images("icon-image", &properties, zoom, &asked)
                .filter(|icon| !icon.is_empty())
            {
                icons.insert(icon);
            }
            icons.extend(asked.0.into_inner());
            icons.extend(
                paint
                    .label_sections(&properties, zoom)
                    .into_iter()
                    .filter_map(|section| section.image),
            );
        }
    }
    (fonts, icons)
}

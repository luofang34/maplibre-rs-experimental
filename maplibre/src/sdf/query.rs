//! Queries the screen bounds of symbols accepted by the latest placement pass.
use crate::{
    coords::WorldTileCoords, sdf::SymbolLayersDataComponent, style::Style, tcs::world::World,
};
use serde::Serialize;

#[derive(Default, Debug)]
pub(crate) struct PlacedSymbols(pub(crate) Vec<PlacedSymbol>);

#[derive(Debug)]
pub(crate) struct PlacedSymbol {
    pub(crate) coords: WorldTileCoords,
    pub(crate) layer: String,
    pub(crate) feature: usize,
    pub(crate) rectangles: [Option<[f64; 4]>; 2],
}

/// A rendered symbol and its source attributes, ordered from the topmost style layer.
#[derive(Clone, Debug, Serialize)]
pub struct RenderedSymbol {
    /// Style layer ID.
    pub layer: String,
    /// Style source ID.
    pub source: Option<String>,
    /// Vector source layer.
    pub source_layer: String,
    /// Original feature ID, if supplied by the source.
    pub id: Option<u64>,
    /// Original source attributes.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// Rendered text, also useful for accessible selection feedback.
    pub text: String,
    /// Geographic anchor as longitude and latitude in degrees.
    pub coordinates: [f64; 2],
}

/// Returns placed text or icons whose screen bounds contain the point, in pixels with y down.
/// An optional layer list restricts results. Hidden collision candidates are excluded.
pub fn query_rendered_symbols(
    world: &World,
    style: &Style,
    point: [f64; 2],
    layers: Option<&[&str]>,
) -> Vec<RenderedSymbol> {
    if !point.iter().all(|v| v.is_finite()) {
        return Vec::new();
    }
    let Some(placed) = world.resources.get::<PlacedSymbols>() else {
        return Vec::new();
    };
    let mut matches = Vec::new();
    for hit in &placed.0 {
        if layers.is_some_and(|layers| !layers.contains(&hit.layer.as_str()))
            || !hit.rectangles.iter().flatten().any(|r| {
                point[0] >= r[0] && point[0] <= r[2] && point[1] >= r[1] && point[1] <= r[3]
            })
        {
            continue;
        }
        let Some(layer) = world
            .tiles
            .query::<&SymbolLayersDataComponent>(hit.coords)
            .and_then(|component| {
                component
                    .layers
                    .iter()
                    .find(|layer| layer.style_layer_id == hit.layer)
            })
        else {
            continue;
        };
        let Some(feature) = layer.features.get(hit.feature) else {
            continue;
        };
        let Some(style_layer) = style.layers.iter().find(|layer| layer.id == hit.layer) else {
            continue;
        };
        let scale = 2_f64.powi(i32::from(u8::from(hit.coords.z)));
        let x = (f64::from(hit.coords.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale;
        let y = (f64::from(hit.coords.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale;
        matches.push((
            style_layer.index,
            feature.data.sort_key,
            RenderedSymbol {
                layer: hit.layer.clone(),
                source: style_layer.source.clone(),
                source_layer: layer.source_layer.clone(),
                id: feature.data.id,
                properties: feature
                    .data
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
                text: feature.str.clone(),
                coordinates: [
                    (x * 360.0 + 180.0).rem_euclid(360.0) - 180.0,
                    (std::f64::consts::PI * (1.0 - 2.0 * y))
                        .sinh()
                        .atan()
                        .to_degrees(),
                ],
            },
        ));
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.total_cmp(&a.1)));
    matches.into_iter().map(|(_, _, feature)| feature).collect()
}

#[cfg(test)]
mod tests;

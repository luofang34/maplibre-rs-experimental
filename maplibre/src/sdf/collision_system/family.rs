//! Layers GL JS groups into one symbol bucket share one placement.
//!
//! GL JS builds one bucket for style layers alike in type, source, source layer, zoom range,
//! filter and layout, and only the lowest of them places it. The others draw that bucket with
//! their own paint and the same placement, so they neither collide with one another nor
//! with the lowest, and a query finds a placed label in each of them.

use std::collections::HashMap;

use super::layer_pass::{empty_metadata, write_feature_metadata};
use crate::{
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{
        line_glyphs::GlyphPose,
        query::{PlacedSymbol, PlacedSymbols},
        SymbolLayerData,
    },
    style::Style,
};

/// How one feature was placed, for the layers that share its placement.
pub(super) struct Outcome {
    pub(super) opacity: [f32; 2],
    pub(super) text_shift: [f32; 2],
    pub(super) anchor: usize,
    pub(super) ground: f32,
    pub(super) poses: Option<Vec<GlyphPose>>,
}

/// Each tile's features as placed, in the order of the layer's tiles.
pub(super) type Outcomes = Vec<Vec<Option<Outcome>>>;

/// The layer that places for each symbol layer GL JS would group with others: the lowest of
/// its group in style order. Layers that are alone are absent.
pub(super) fn leaders(style: &Style) -> HashMap<String, String> {
    let mut groups: HashMap<String, Vec<(u32, &str)>> = HashMap::new();
    for layer in style.layers.iter().filter(|layer| layer.type_ == "symbol") {
        if let Some(key) = layer.layout_group_key() {
            groups
                .entry(key)
                .or_default()
                .push((layer.index, &layer.id));
        }
    }
    let mut leaders = HashMap::new();
    for members in groups.values().filter(|members| members.len() > 1) {
        let Some((_, lowest)) = members.iter().min_by_key(|(index, _)| *index) else {
            continue;
        };
        for (_, id) in members {
            leaders.insert((*id).to_owned(), (*lowest).to_owned());
        }
    }
    leaders
}

/// The metadata of `members` drawn with their own paint and the placement of `leader`'s
/// tiles, and their labels added to the queryable ones under their own layer.
pub(super) fn mirror(
    (leader, outcomes): (&[&SymbolLayerData], &Outcomes),
    members: &[&SymbolLayerData],
    (paint, zoom): (&crate::style::layer::SymbolPaint, f64),
    placed: &mut PlacedSymbols,
) -> Vec<Vec<SDFShaderFeatureMetadata>> {
    let leader_id = leader.first().map(|layer| layer.style_layer_id.clone());
    let copies: Vec<PlacedSymbol> = placed
        .0
        .iter()
        .filter(|symbol| Some(&symbol.layer) == leader_id.as_ref())
        .flat_map(|symbol| {
            members
                .iter()
                .filter(|member| member.coords == symbol.coords)
                .map(|member| PlacedSymbol {
                    layer: member.style_layer_id.clone(),
                    ..symbol.clone()
                })
        })
        .collect();
    placed.0.extend(copies);
    members
        .iter()
        .map(|member| {
            let mut metadata = empty_metadata(member);
            let Some(tile) = leader
                .iter()
                .position(|layer| layer.coords == member.coords)
            else {
                return metadata;
            };
            for (index, outcome) in outcomes[tile].iter().enumerate() {
                let (Some(outcome), Some(feature)) = (outcome, member.features.get(index)) else {
                    continue;
                };
                write_feature_metadata(
                    member,
                    feature,
                    (outcome.opacity, outcome.text_shift, outcome.anchor),
                    outcome.ground,
                    outcome.poses.as_deref(),
                    (paint, zoom, &mut metadata),
                );
            }
            metadata
        })
        .collect()
}

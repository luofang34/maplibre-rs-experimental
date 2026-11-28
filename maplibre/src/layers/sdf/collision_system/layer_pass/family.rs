//! Layers GL JS groups into one symbol bucket share one placement.
//!
//! GL JS builds one bucket for style layers alike in type, source, source layer, zoom range,
//! filter and layout, and only the lowest of them places it. The others draw that bucket with
//! their own paint and the same placement, so they neither collide with one another nor
//! with the lowest, and a query finds a placed label in each of them.

use rustc_hash::FxHashMap as HashMap;

use crate::{sdf::line_glyphs::GlyphPose, style::Style};

/// How one feature was placed, for the layers that share its placement.
pub(in crate::layers::sdf::collision_system) struct Outcome {
    pub(in crate::layers::sdf::collision_system) ramps: [super::super::temporal::FadeRamp; 2],
    pub(in crate::layers::sdf::collision_system) text_shift: [f32; 2],
    pub(in crate::layers::sdf::collision_system) anchor: usize,
    pub(in crate::layers::sdf::collision_system) ground: f32,
    pub(in crate::layers::sdf::collision_system) poses: Option<Vec<GlyphPose>>,
}

/// Each tile's features as placed, in the order of the layer's tiles.
pub(in crate::layers::sdf::collision_system) type Outcomes = Vec<Vec<Option<Outcome>>>;

/// The layer that places for each symbol layer GL JS would group with others: the lowest of
/// its group in style order. Layers that are alone are absent.
pub(in crate::layers::sdf::collision_system) fn leaders(style: &Style) -> HashMap<String, String> {
    let mut groups: HashMap<String, Vec<(u32, &str)>> = HashMap::default();
    for layer in style.layers.iter().filter(|layer| layer.type_ == "symbol") {
        if let Some(key) = layer.layout_group_key() {
            groups
                .entry(key)
                .or_default()
                .push((layer.index, &layer.id));
        }
    }
    let mut leaders = HashMap::default();
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

#[cfg(test)]
mod tests;

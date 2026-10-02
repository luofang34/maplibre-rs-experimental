//! Labels that may overlap are drawn in the order GL JS sorts them for the current bearing.
//!
//! Tessellation orders a layer's index buffer by unrotated height. GL JS sorts the labels again
//! by their height on the rotated screen whenever the bearing changes, so the lower of two
//! overlapping labels covers the other however the map is turned; the index buffer of each such
//! layer is rewritten here when the bearing it was sorted for changes.

use std::collections::HashMap;

use crate::{
    coords::WorldTileCoords,
    render::eventually::{Eventually, Eventually::Initialized},
    sdf::{
        tessellation::{drawn_order, sorts_by_height},
        SymbolBufferPool, SymbolLayerData,
    },
    style::layer::SymbolPaint,
};

/// The bearing each layer's indices were last sorted for, by tile and allocation.
#[derive(Default)]
pub(super) struct DrawSort {
    sorted: HashMap<(WorldTileCoords, String), (u64, u64)>,
}

impl DrawSort {
    /// Rewrites the indices of the `layers` that sort by height and were sorted for another
    /// bearing; an upload that has not been sorted yet holds the unrotated order.
    pub(super) fn follow_bearing(
        &mut self,
        world: &crate::tcs::world::World,
        queue: &crate::render::upload_queue::UploadQueue,
        (bearing, zoom): (f64, f64),
        layers: &[(u32, &SymbolLayerData, &SymbolPaint)],
    ) {
        let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
            return;
        };
        // Uploads that left the pool take their entries with them.
        self.sorted
            .retain(|(coords, _), _| pool.index().get_layers(*coords).is_some());
        for (_, layer, paint) in layers {
            if !sorts_by_height(paint, zoom) {
                continue;
            }
            let Some(entry) = pool.index().get_layers(layer.coords).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.style_layer.id == layer.style_layer_id)
            }) else {
                continue;
            };
            let key = (layer.coords, layer.style_layer_id.clone());
            let sorted_for = self
                .sorted
                .get(&key)
                .filter(|(allocation, _)| *allocation == entry.allocation_id())
                .map_or(0.0_f64.to_bits(), |(_, bearing)| *bearing);
            if sorted_for != bearing.to_bits() {
                if let Some(indices) = indices_in_order(layer, bearing) {
                    pool.update_indices(queue, entry, &indices);
                }
            }
            self.sorted
                .insert(key, (entry.allocation_id(), bearing.to_bits()));
        }
    }
}

/// The layer's indices with each label's run in drawn order; `None` when the runs do not
/// cover the buffer as tessellation leaves them.
fn indices_in_order(layer: &SymbolLayerData, bearing: f64) -> Option<Vec<u32>> {
    let source = &layer.buffer.buffer.indices;
    let covered: usize = layer
        .features
        .iter()
        .map(|feature| feature.indices.len())
        .sum();
    if covered != source.len() {
        return None;
    }
    let mut indices = Vec::with_capacity(source.len());
    for index in drawn_order(&layer.features, bearing) {
        indices.extend_from_slice(source.get(layer.features[index].indices.clone())?);
    }
    Some(indices)
}

#[cfg(test)]
mod tests;

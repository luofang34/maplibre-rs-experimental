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

/// Discrete angular steps around `2π` (`96` steps = `3.75°` per bucket, hitting `0°`, `45°`,
/// `90°`, `180°`, and `270°` exactly while suppressing sub-degree orbit index-buffer rewrites).
const BEARING_BUCKETS: u16 = 96;

fn bearing_bucket(bearing: f64) -> u16 {
    if !bearing.is_finite() {
        return 0;
    }
    let normalized = bearing.rem_euclid(std::f64::consts::TAU);
    ((normalized / std::f64::consts::TAU) * f64::from(BEARING_BUCKETS)).round() as u16
        % BEARING_BUCKETS
}

fn bucket_angle_rad(bucket: u16) -> f64 {
    (f64::from(bucket) / f64::from(BEARING_BUCKETS)) * std::f64::consts::TAU
}

/// The quantized bearing bucket each layer's indices were last sorted for, by tile and allocation.
#[derive(Default)]
pub(super) struct DrawSort {
    sorted: HashMap<(WorldTileCoords, String), (u64, u16)>,
}

impl DrawSort {
    /// Returns `true` when any height-sorted symbol bucket in `layers` has not yet been ordered
    /// for `bearing`'s discrete angular bucket.
    pub(super) fn needs_bearing_sort(
        &self,
        world: &crate::tcs::world::World,
        (bearing, zoom): (f64, f64),
        layers: &[(u32, &SymbolLayerData, &SymbolPaint)],
    ) -> bool {
        let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
            return false;
        };
        let bucket = bearing_bucket(bearing);
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
            let sorted_for = self
                .sorted
                .get(&(layer.coords, layer.style_layer_id.clone()))
                .filter(|(allocation, _)| *allocation == entry.allocation_id())
                .map_or(0_u16, |(_, b)| *b);
            if sorted_for != bucket {
                return true;
            }
        }
        false
    }

    /// Rewrites the indices of the `layers` that sort by height and were sorted for another
    /// bearing bucket; an upload that has not been sorted yet holds the unrotated (`0`) order.
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
        let bucket = bearing_bucket(bearing);
        let sort_bearing = bucket_angle_rad(bucket);
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
                .map_or(0_u16, |(_, b)| *b);
            if sorted_for != bucket {
                if let Some(indices) = indices_in_order(layer, sort_bearing) {
                    pool.update_indices(queue, entry, &indices);
                }
            }
            self.sorted.insert(key, (entry.allocation_id(), bucket));
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

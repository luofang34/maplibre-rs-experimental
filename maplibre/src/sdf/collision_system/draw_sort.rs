//! Labels that may overlap are drawn in the order GL JS sorts them for the current bearing.
//!
//! Tessellation orders a layer's index buffer by unrotated height. GL JS sorts the labels again
//! by their height on the rotated screen whenever the bearing changes, so the lower of two
//! overlapping labels covers the other however the map is turned; the index buffer of each such
//! layer is rewritten here when the bearing it was sorted for changes.

use rustc_hash::FxHashMap as HashMap;

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
    sorted: HashMap<(WorldTileCoords, crate::style::StyleLayerId), (u64, u16)>,
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
        let bucket = bearing_bucket(bearing);
        if self.sorted.is_empty() && bucket == 0 {
            return false;
        }
        let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
            return false;
        };
        for (_, layer, paint) in layers {
            if !sorts_by_height(paint, zoom) {
                continue;
            }
            let layer_id = crate::style::StyleLayerId::intern(&layer.style_layer_id);
            let Some(entry) = pool.index().get_layers(layer.coords).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.style_layer_id == layer_id)
            }) else {
                continue;
            };
            let sorted_for = self
                .sorted
                .get(&(layer.coords, layer_id))
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
    /// Returns `true` when `budget` stopped work early with more buckets still needing sorting.
    pub(super) fn follow_bearing(
        &mut self,
        world: &crate::tcs::world::World,
        queue: &crate::render::upload_queue::UploadQueue,
        (bearing, zoom): (f64, f64),
        layers: &[(u32, &SymbolLayerData, &SymbolPaint)],
        budget: Option<(std::time::Duration, usize)>,
    ) -> bool {
        let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() else {
            return false;
        };
        let bucket = bearing_bucket(bearing);
        let sort_bearing = bucket_angle_rad(bucket);
        let started = instant::Instant::now();
        let mut sorted_this_call = 0_usize;
        // Uploads that left the pool take their entries with them.
        self.sorted
            .retain(|(coords, _), _| pool.index().get_layers(*coords).is_some());
        for (_, layer, paint) in layers {
            if !sorts_by_height(paint, zoom) {
                continue;
            }
            let layer_id = crate::style::StyleLayerId::intern(&layer.style_layer_id);
            let Some(entry) = pool.index().get_layers(layer.coords).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.style_layer_id == layer_id)
            }) else {
                continue;
            };
            let key = (layer.coords, layer_id);
            let sorted_for = self
                .sorted
                .get(&key)
                .filter(|(allocation, _)| *allocation == entry.allocation_id())
                .map_or(0_u16, |(_, b)| *b);
            if sorted_for != bucket {
                if let Some((max_time, max_buckets)) = budget {
                    if sorted_this_call > 0
                        && (sorted_this_call >= max_buckets || started.elapsed() >= max_time)
                    {
                        return true;
                    }
                }
                if let Some(indices) = indices_in_order(layer, sort_bearing) {
                    pool.update_indices(queue, entry, &indices);
                }
                sorted_this_call += 1;
            }
            self.sorted.insert(key, (entry.allocation_id(), bucket));
        }
        false
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

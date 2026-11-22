//! In-place GPU refresh for dynamic style layer order, visibility, paint, and `feature-state`.

mod feature_state;
mod vector_pool;

use feature_state::recolor_cpu_bucket;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use vector_pool::refresh_vector_pool;

use super::{
    metadata::{feature_metadata, metadata_for_layer, pattern_runs},
    paint, VectorPaintFrame,
};
use crate::{
    coords::WorldTileCoords,
    io::tile_retry::{self, RequestKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        shaders::FillShaderFeatureMetadata,
        upload_queue::UploadQueue,
    },
    sdf::SymbolBufferPool,
    style::{
        feature_state::{
            layer_geometry_reads_feature_state, layer_paint_reads_feature_state,
            FeatureStateArcMemo,
        },
        layer::LayerPaint,
        Style,
    },
    tcs::world::World,
    vector::{
        content::LayerReplacements, feature_paint::TileFeaturePaintComponent,
        pattern::per_feature_pattern, VectorBufferPool, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

/// Queued in-place style and `feature-state` mutations awaiting the next GPU upload pass.
#[derive(Default, Debug)]
pub(crate) struct DirtyVectorStyle {
    /// Sync `StyleLayer` metadata (`index`, zoom range, visibility, uniforms) on GPU pools.
    pub sync_layers: bool,
    /// Style layer IDs whose per-feature colors/opacity/outline may have changed.
    pub paint_layers: HashSet<String>,
    /// Style source names whose runtime `feature-state` changed.
    pub feature_state_sources: HashSet<String>,
}

impl DirtyVectorStyle {
    /// Queues synchronization of layer order, zoom bounds, visibility, and paint for `layers`.
    pub fn mark_style_edit(world: &mut World, paint_layers: impl IntoIterator<Item = String>) {
        let dirty = world.resources.get_or_init_mut::<Self>();
        dirty.sync_layers = true;
        dirty.paint_layers.extend(paint_layers);
    }

    /// Queues an in-place `feature-state` update for `source`.
    pub fn mark_feature_state(world: &mut World, source: impl Into<String>) {
        let dirty = world.resources.get_or_init_mut::<Self>();
        dirty.feature_state_sources.insert(source.into());
    }
}

pub(super) fn apply_dirty_style(
    world: &mut World,
    style: &Style,
    queue: &UploadQueue,
    frame: VectorPaintFrame,
) -> bool {
    let Some(dirty) = world.resources.remove::<DirtyVectorStyle>() else {
        return false;
    };
    let mut needs_worker_refresh = false;
    let mut recolor_layers = dirty.paint_layers;

    if !dirty.feature_state_sources.is_empty() {
        let mut memo = FeatureStateArcMemo::default();
        for source in &dirty.feature_state_sources {
            world
                .tiles
                .geometry_index
                .sync_source_feature_state_with_memo(
                    source,
                    style.source_promote_id(source),
                    style.feature_state.source(source),
                    &mut memo,
                );
        }
        for layer in &style.layers {
            let Some(source) = layer.source.as_deref() else {
                continue;
            };
            if !dirty.feature_state_sources.contains(source) {
                continue;
            };
            if layer_geometry_reads_feature_state(layer) {
                needs_worker_refresh = true;
            }
            if layer.type_ != "symbol" && layer_paint_reads_feature_state(layer) {
                recolor_layers.insert(layer.id.clone());
            }
        }
        feature_state::sync_tile_feature_states(
            world,
            style,
            &dirty.feature_state_sources,
            &mut memo,
            &mut needs_worker_refresh,
        );
    }

    if dirty.sync_layers {
        sync_symbol_pool_layers(world, style);
    }

    if dirty.sync_layers || !recolor_layers.is_empty() {
        refresh_vector_pool(
            world,
            style,
            queue,
            frame,
            dirty.sync_layers,
            &recolor_layers,
            &mut needs_worker_refresh,
        );
    }

    if needs_worker_refresh {
        tile_retry::refresh(world, RequestKind::Vector);
    }
    true
}

fn sync_symbol_pool_layers(world: &mut World, style: &Style) {
    let Some(Initialized(symbol_pool)) = world.resources.get_mut::<Eventually<SymbolBufferPool>>()
    else {
        return;
    };
    for style_layer in &style.layers {
        if style_layer.type_ != "symbol" {
            continue;
        }
        drop(symbol_pool.sync_style_layer(style_layer));
    }
}

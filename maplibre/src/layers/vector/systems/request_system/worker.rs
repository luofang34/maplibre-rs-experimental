//! Source fetching and completion of vector worker requests.

use std::sync::Arc;

use rustc_hash::FxHashSet as HashSet;

use super::{
    group_tile::{decode_group_tile, fetch_raw_group_tile, GroupTile},
    limiter::acquire_heavy_tile_slot,
};

mod provided;
mod source;
use source::process_source;

use crate::{
    coords::WorldTileCoords,
    environment::OffscreenKernel,
    io::{
        apc::{
            run_cpu_bound_replying, AsyncProcedureFuture, AttemptContext, Context, Input,
            IntoMessage, ProcedureError, SendError,
        },
        geometry_index::{LazyMvtIndex, TileIndex},
        source_client::{HttpClient, SourceClient},
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    sdf::assets::{
        fetch_symbol_asset_inputs, pack_symbol_assets, symbol_requests_from_tile,
        SymbolAssetConfig, SymbolAtlas,
    },
    style::{layer::StyleLayer, Style},
    vector::{
        process_vector::process_decoded_vector_tile_with_assets,
        transferables::{LayerIndexed, LayerMissing, TileTessellated, VectorTransferables},
        ProcessVectorContext, ProcessVectorError, VectorTileRequest,
    },
};

/// Fetches, decodes, tessellates, and shapes symbols for one vector tile on a worker.
pub fn fetch_vector_apc<K: OffscreenKernel, T: VectorTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> AsyncProcedureFuture {
    Box::pin(run_vector_apc::<K, T, C>(input, context, kernel))
}

async fn run_vector_apc<K: OffscreenKernel, T: VectorTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> Result<(), ProcedureError> {
    let Input {
        coords,
        style,
        attempt,
        overscaled_zoom,
        pixel_ratio,
        only_sources,
        refetch,
    } = input;
    let context = AttemptContext::new(context, attempt);
    let client = kernel.source_client();
    let only_sources: Option<HashSet<String>> =
        only_sources.map(|sources| sources.into_iter().collect());
    let group_list = collect_worker_groups(&style, coords, overscaled_zoom, only_sources.as_ref());
    let mut groups = group_list.into_iter().peekable();
    let mut failed = false;
    let mut retry = false;
    let mut images = provided::TileImages::new(coords, Some(attempt), pixel_ratio);
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    let mut shared_permit: Option<super::limiter::SharedTilePermit> = None;
    let mut state = GroupRunState {
        failed: &mut failed,
        retry: &mut retry,
        images: &mut images,
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        shared_permit: &mut shared_permit,
    };
    while let Some(group) = groups.next() {
        let last_source = groups.peek().is_none() && !*state.failed;
        run_single_group::<T, _, K::HttpClient>(
            &client,
            GroupRunTarget {
                coords,
                style: &style,
                group: &group,
                refetch,
                overscaled_zoom,
                pixel_ratio,
                last_source,
            },
            context.clone(),
            &mut state,
        )
        .await?;
    }
    images
        .report_first(&context)
        .map_err(ProcedureError::Send)?;
    finish::<T, _>(
        &context,
        coords,
        Some(attempt),
        (failed, retry),
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        shared_permit,
    )?;
    // The tile is complete and drawn; its labels are laid out again as images arrive.
    images
        .lay_out_again::<T, _, K::HttpClient>(&client, &style, context)
        .await
        .map_err(ProcedureError::Send)
}

fn collect_worker_groups(
    style: &Style,
    coords: WorldTileCoords,
    overscaled_zoom: u8,
    only_sources: Option<&HashSet<String>>,
) -> Vec<SourceLayerGroup> {
    // A request for a change to some sources' data leaves the tile's other sources as the
    // tile shows them.
    let has_terrain = style.terrain.is_some();
    let mut group_list: Vec<_> = source_layer_groups(style, TileKind::Vector)
        .into_iter()
        .filter_map(|mut group| {
            let included = only_sources.is_none_or(|only| {
                group
                    .source_name
                    .as_ref()
                    .is_some_and(|name| only.contains(name))
            });
            if !included {
                return None;
            }
            group
                .layers
                .retain(|layer| layer.needed_for_tile(has_terrain, coords, overscaled_zoom));
            (!group.layers.is_empty()).then_some(group)
        })
        .collect();
    prioritize_remote_sources(&mut group_list);
    group_list
}

struct GroupRunState<'a> {
    failed: &'a mut bool,
    retry: &'a mut bool,
    images: &'a mut provided::TileImages,
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    shared_permit: &'a mut Option<super::limiter::SharedTilePermit>,
}

struct GroupRunTarget<'a> {
    coords: WorldTileCoords,
    style: &'a Style,
    group: &'a SourceLayerGroup,
    refetch: bool,
    overscaled_zoom: u8,
    pixel_ratio: f32,
    last_source: bool,
}

async fn run_single_group<T: VectorTransferables, C: Context + Clone + Send, H: HttpClient>(
    client: &SourceClient<H>,
    target: GroupRunTarget<'_>,
    context: C,
    state: &mut GroupRunState<'_>,
) -> Result<(), ProcedureError> {
    let Some((tile, slot)) =
        fetch_and_decode_group_tile::<T, _, _>(client, &target, context.clone(), state).await?
    else {
        return Ok(());
    };
    let mut heavy_slot = Some(slot);
    let pass = SourcePass {
        last_source: target.last_source,
        overscaled_zoom: target.overscaled_zoom,
        pixel_ratio: target.pixel_ratio,
    };
    match process_source::<T, _, H>(
        tile,
        target.coords,
        target.style,
        client,
        target.group,
        context.clone(),
        (
            source::SourceSlots {
                heavy: &mut heavy_slot,
                #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
                shared: state.shared_permit,
            },
            pass,
        ),
    )
    .await
    {
        Ok(symbols) => state.images.add(symbols),
        Err(ProcessVectorError::SendError(source)) => return Err(ProcedureError::Send(source)),
        Err(error @ ProcessVectorError::SymbolAssets(_)) => {
            tracing::warn!(coords = %target.coords, source = ?target.group.source_name, error = %error, "symbol assets unavailable");
            *state.retry = true;
        }
        Err(error @ (ProcessVectorError::Decoding { .. } | ProcessVectorError::CpuWork(_))) => {
            tracing::warn!(coords = %target.coords, source = ?target.group.source_name, %error, "invalid vector tile");
            *state.failed = true;
            source_failed::<T, _>(target.coords, &target.group.layers, &context)
                .map_err(ProcedureError::Send)?;
        }
    }
    Ok(())
}

async fn fetch_and_decode_group_tile<
    T: VectorTransferables,
    C: Context + Clone + Send,
    H: HttpClient,
>(
    client: &SourceClient<H>,
    target: &GroupRunTarget<'_>,
    context: C,
    state: &mut GroupRunState<'_>,
) -> Result<Option<(GroupTile, super::limiter::HeavyTileSlot)>, ProcedureError> {
    if target.group.has_nothing_at(target.coords) {
        source_empty::<T, _>(target.coords, &target.group.layers, &context)
            .map_err(ProcedureError::Send)?;
        return Ok(None);
    }
    let raw_tile = match fetch_raw_group_tile(client, target.coords, target.group, target.refetch)
        .await
    {
        Ok(raw_tile) => raw_tile,
        Err(error) if error.is_not_found() => {
            tracing::debug!(coords = %target.coords, source = ?target.group.source_name, "no vector tile at the source; its layers are empty");
            source_empty::<T, _>(target.coords, &target.group.layers, &context)
                .map_err(ProcedureError::Send)?;
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(coords = %target.coords, source = ?target.group.source_name, error = %error.describe(), "vector tile unavailable");
            *state.failed = true;
            *state.retry |= error.is_retryable();
            source_failed::<T, _>(target.coords, &target.group.layers, &context)
                .map_err(ProcedureError::Send)?;
            return Ok(None);
        }
    };
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    if state.shared_permit.is_none() {
        *state.shared_permit = Some(super::limiter::acquire_shared_tile_permit().await);
    }
    let heavy_slot = acquire_heavy_tile_slot().await;
    let promote_id = target
        .group
        .source_name
        .as_deref()
        .and_then(|name| target.style.source_promote_id(name));
    match decode_group_tile(
        client,
        raw_tile,
        target.coords,
        &target.group.layers,
        promote_id,
    ) {
        Ok(tile) => Ok(Some((tile, heavy_slot))),
        Err(error) => {
            tracing::warn!(coords = %target.coords, source = ?target.group.source_name, error = %error.describe(), "vector tile unavailable");
            *state.failed = true;
            *state.retry |= error.is_retryable();
            source_failed::<T, _>(target.coords, &target.group.layers, &context)
                .map_err(ProcedureError::Send)?;
            Ok(None)
        }
    }
}

/// How one source of a tile is processed.
struct SourcePass {
    /// Whether no other source follows, so this one completes the tile.
    last_source: bool,
    /// The zoom the tile is magnified to, or zero.
    overscaled_zoom: u8,
    /// Device pixels per layout pixel, which provided images are made for.
    pixel_ratio: f32,
}

/// Reports every layer of a source as missing from the tile, which clears what an earlier
/// generation drew there.
fn source_empty<T: VectorTransferables, C: Context>(
    coords: WorldTileCoords,
    layers: &[StyleLayer],
    context: &C,
) -> Result<(), SendError> {
    for layer in layers {
        if let Some(source_layer) = &layer.source_layer {
            context.send_back(T::LayerMissing::build_from(coords, source_layer.clone()))?;
        }
    }
    Ok(())
}

fn source_failed<T: VectorTransferables, C: Context>(
    coords: WorldTileCoords,
    layers: &[StyleLayer],
    context: &C,
) -> Result<(), SendError> {
    source_empty::<T, C>(coords, layers, context)?;
    context.send_back(T::TileTessellated::build_failed(coords, true))
}

/// Tessellates the requested layers of one source's decoded tile off the fetching threads,
/// replying with their geometry as it is made.
async fn tessellate<T: VectorTransferables, C: Context + Send>(
    context: C,
    (emit_partial, index_geometry, has_dem_source): (bool, bool, bool),
    (tile, request): (
        Arc<crate::vector::process_vector::DecodedVectorTile>,
        VectorTileRequest,
    ),
    (promote_id, feature_state): (
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
    atlas: Arc<SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    run_cpu_bound_replying(context, move |context| {
        let mut processor = source_processor::<T, _>(context, emit_partial)
            .with_has_dem_source(has_dem_source)
            .with_source_feature_state(promote_id, feature_state);
        if !index_geometry {
            processor = processor.without_indexing();
        }
        process_decoded_vector_tile_with_assets(&tile, request, &mut processor, atlas)
    })
    .await
    .map_err(ProcessVectorError::CpuWork)?
}

fn source_processor<T: VectorTransferables, C: Context>(
    context: C,
    emit_partial: bool,
) -> ProcessVectorContext<T, C> {
    let processor = ProcessVectorContext::new(context).with_pending_symbols();
    if emit_partial {
        processor
    } else {
        processor.without_completion()
    }
}

fn finish<T: VectorTransferables, C: Context>(
    context: &C,
    coords: WorldTileCoords,
    attempt: Option<u64>,
    (failed, retry): (bool, bool),
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))] permit: Option<
        super::limiter::SharedTilePermit,
    >,
) -> Result<(), ProcedureError> {
    let completion = if failed {
        T::TileTessellated::build_failed(coords, false)
    } else {
        T::TileTessellated::build_from(coords)
    };
    context
        .send_back(completion)
        .map_err(ProcedureError::Send)?;
    let outcome = IntoMessage::into(TileRequestOutcome {
        coords,
        kind: RequestKind::Vector,
        attempt,
        disposition: if retry {
            RequestDisposition::Retry
        } else {
            RequestDisposition::Complete
        },
    });
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    let outcome = match permit {
        Some(permit) => outcome.with_shared_permit(permit),
        None => outcome,
    };
    context.send_back(outcome).map_err(ProcedureError::Send)
}

fn prioritize_remote_sources(groups: &mut [SourceLayerGroup]) {
    groups.sort_by_key(|group| {
        matches!(group.source, crate::io::source_type::SourceType::GeoJson(_))
    });
}

#[cfg(test)]
mod tests;

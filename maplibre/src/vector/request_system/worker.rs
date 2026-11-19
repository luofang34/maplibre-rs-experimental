//! Source fetching and completion of vector worker requests.

use std::sync::Arc;

use rustc_hash::FxHashSet as HashSet;

use super::{
    group_tile::{decode_group_tile, fetch_raw_group_tile, GroupTile},
    limiter::acquire_heavy_tile_slot,
};

mod provided;
use crate::{
    coords::WorldTileCoords,
    environment::OffscreenKernel,
    io::{
        apc::{
            AsyncProcedureFuture, AttemptContext, Context, Input, IntoMessage, ProcedureError,
            SendError,
        },
        cpu_work::run_cpu_bound_replying,
        geometry_index::{LazyMvtIndex, TileIndex},
        source_client::{HttpClient, SourceClient},
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    sdf::assets::{
        load_symbol_assets_for_requests, symbol_requests_from_tile, SymbolAssetConfig, SymbolAtlas,
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
    Box::pin(async move {
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
        // A request for a change to some sources' data leaves the tile's other sources as the
        // tile shows them.
        let has_terrain = style.terrain.is_some();
        let mut groups = source_layer_groups(&style, TileKind::Vector)
            .into_iter()
            .filter_map(|mut group| {
                let included = only_sources.as_ref().is_none_or(|only| {
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
            .peekable();
        let mut failed = false;
        let mut retry = false;
        let mut images = provided::TileImages::new(coords, Some(attempt), pixel_ratio);
        while let Some(group) = groups.next() {
            if group.has_nothing_at(coords) {
                source_empty::<T, _>(coords, &group.layers, &context)
                    .map_err(ProcedureError::Send)?;
                continue;
            }
            enum FetchStep {
                Ready(super::group_tile::RawGroupTile),
                NotFound,
                Failed { retryable: bool },
            }
            let fetch_step = match fetch_raw_group_tile(&client, coords, &group, refetch).await {
                Ok(raw_tile) => FetchStep::Ready(raw_tile),
                Err(error) if error.is_not_found() => {
                    tracing::debug!(%coords, source = ?group.source_name, "no vector tile at the source; its layers are empty");
                    FetchStep::NotFound
                }
                Err(error) => {
                    tracing::warn!(%coords, source = ?group.source_name, error = %error.describe(), "vector tile unavailable");
                    FetchStep::Failed {
                        retryable: error.is_retryable(),
                    }
                }
            };
            let raw_tile = match fetch_step {
                FetchStep::Ready(raw_tile) => raw_tile,
                // A source without a tile there has nothing to draw, as GL JS loads a vector
                // tile that answered 404 empty: its layers are empty, and the tile's other
                // sources are drawn as usual.
                FetchStep::NotFound => {
                    source_empty::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                    continue;
                }
                FetchStep::Failed { retryable } => {
                    failed = true;
                    retry |= retryable;
                    source_failed::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                    continue;
                }
            };
            super::limiter::wait_for_shared_tile_slot().await;
            let mut heavy_slot = Some(acquire_heavy_tile_slot().await);
            let promote_id = group
                .source_name
                .as_deref()
                .and_then(|name| style.source_promote_id(name));
            let tile = match decode_group_tile(&client, raw_tile, coords, &group.layers, promote_id)
            {
                Ok(tile) => tile,
                Err(error) => {
                    tracing::warn!(%coords, source = ?group.source_name, error = %error.describe(), "vector tile unavailable");
                    failed = true;
                    retry |= error.is_retryable();
                    source_failed::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                    continue;
                }
            };
            match process_source::<T, _, K::HttpClient>(
                tile,
                coords,
                &style,
                &client,
                &group,
                context.clone(),
                (
                    &mut heavy_slot,
                    SourcePass {
                        last_source: groups.peek().is_none() && !failed,
                        overscaled_zoom,
                        pixel_ratio,
                    },
                ),
            )
            .await
            {
                Ok(symbols) => images.add(symbols),
                Err(ProcessVectorError::SendError(source)) => {
                    return Err(ProcedureError::Send(source))
                }
                Err(error @ ProcessVectorError::SymbolAssets(_)) => {
                    // The base layers were delivered, so the tile stays complete and drawn without
                    // labels; only the retry outcome asks for another attempt.
                    tracing::warn!(%coords, source = ?group.source_name, error = %error, "symbol assets unavailable");
                    retry = true;
                }
                Err(
                    error @ (ProcessVectorError::Decoding { .. } | ProcessVectorError::CpuWork(_)),
                ) => {
                    tracing::warn!(%coords, source = ?group.source_name, %error, "invalid vector tile");
                    failed = true;
                    source_failed::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                }
            }
        }
        images
            .report_first(&context)
            .map_err(ProcedureError::Send)?;
        finish::<T, _>(&context, coords, Some(attempt), failed, retry)?;
        // The tile is complete and drawn; its labels are laid out again as images arrive.
        images
            .lay_out_again::<T, _, K::HttpClient>(&client, &style, context)
            .await
            .map_err(ProcedureError::Send)
    })
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

async fn process_source<T: VectorTransferables, C: Context + Clone + Send, H: HttpClient>(
    group_tile: GroupTile,
    coords: WorldTileCoords,
    style: &Style,
    client: &SourceClient<H>,
    group: &SourceLayerGroup,
    context: C,
    (heavy_slot, pass): (&mut Option<super::limiter::HeavyTileSlot>, SourcePass),
) -> Result<Option<provided::Symbols>, ProcessVectorError> {
    let GroupTile {
        mut tile,
        mvt_bytes,
        mvt_compressed,
        mvt_rescale,
    } = group_tile;
    let SourcePass {
        last_source,
        overscaled_zoom,
        pixel_ratio,
    } = pass;
    let projection = style
        .projection
        .as_ref()
        .map_or_else(Default::default, |specification| {
            specification.projection_type.clone()
        });
    let (symbols, base): (HashSet<_>, HashSet<_>) = group
        .layers
        .iter()
        .cloned()
        .partition(|layer| layer.type_ == "symbol");
    let source_state = group.source_name.as_deref().map_or((None, None), |name| {
        (
            style.source_promote_id(name).cloned(),
            style.feature_state.source(name).cloned(),
        )
    });
    if T::LayerIndexed::requires_geometry_index() && (!base.is_empty() || !symbols.is_empty()) {
        let lazy = LazyMvtIndex::from_packed_with_state(
            mvt_bytes,
            mvt_compressed,
            source_state.0.clone(),
            source_state.1.clone(),
        )
        .with_rescale(mvt_rescale);
        context
            .send_back(T::LayerIndexed::build_from(
                coords,
                group.source_name.clone(),
                TileIndex::LazyMvt(lazy),
            ))
            .map_err(ProcessVectorError::SendError)?;
    }
    let request = VectorTileRequest {
        coords,
        layers: base,
        projection: projection.clone(),
        overscaled_zoom,
    };
    let has_dem_source = crate::terrain::source::dem_source(style).is_some();
    if symbols.is_empty() {
        tessellate::<T, C>(
            context,
            (false, false, has_dem_source),
            (tile, request),
            source_state,
            Arc::new(SymbolAtlas::default()),
        )
        .await?;
        let _ = heavy_slot.take();
        return Ok(None);
    }
    let zoom = f64::from(overscaled_zoom.max(u8::from(coords.z)));
    let sym_requests = symbol_requests_from_tile(&symbols, &tile, zoom, Some(style));
    let asset_config = SymbolAssetConfig {
        pixel_ratio,
        ..SymbolAssetConfig::of(style)
    };
    let mut assets_fut = std::pin::pin!(load_symbol_assets_for_requests(
        client,
        asset_config,
        sym_requests
    ));
    let mut pre_assets = None;
    let _ = std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(res) = std::future::Future::poll(assets_fut.as_mut(), cx) {
            pre_assets = Some(res);
        }
        std::task::Poll::Ready(())
    })
    .await;
    tessellate::<T, C>(
        context.clone(),
        (false, false, has_dem_source),
        (tile.clone(), request),
        source_state.clone(),
        Arc::new(SymbolAtlas::default()),
    )
    .await?;

    let sym_request = VectorTileRequest {
        coords,
        layers: symbols,
        projection,
        overscaled_zoom,
    };
    if let Some(crate::vector::process_vector::DecodedVectorTile::Mvt(inner)) =
        Arc::get_mut(&mut tile)
    {
        inner.layers.retain(|mvt| {
            sym_request
                .layers
                .iter()
                .any(|sl| sl.source_layer.as_deref() == Some(mvt.name.as_str()))
        });
    }
    if pre_assets.is_none() {
        let _ = std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(res) = std::future::Future::poll(assets_fut.as_mut(), cx)
            {
                pre_assets = Some(res);
            }
            std::task::Poll::Ready(())
        })
        .await;
    }
    let assets = match pre_assets {
        Some(res) => res,
        None => {
            if last_source {
                context
                    .send_back(T::TileTessellated::build_partial(coords))
                    .map_err(ProcessVectorError::SendError)?;
            }
            // Release HeavyTileSlot strictly BEFORE awaiting remote glyph/sprite HTTP I/O.
            let _ = heavy_slot.take();
            let loaded = assets_fut.await;
            *heavy_slot = Some(acquire_heavy_tile_slot().await);
            loaded
        }
    }
    .map_err(ProcessVectorError::SymbolAssets)?;
    // Only a source still waiting for images keeps its tile and layers to lay out again.
    let again = (!assets.awaiting.is_empty()).then(|| (tile.clone(), sym_request.clone()));
    tessellate::<T, C>(
        context,
        (false, false, has_dem_source),
        (tile, sym_request),
        source_state,
        assets.atlas,
    )
    .await?;
    let _ = heavy_slot.take();
    Ok(Some(provided::Symbols {
        again,
        pixel_ratio,
        provided: assets.provided,
        awaiting: assets.awaiting,
    }))
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
    failed: bool,
    retry: bool,
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
    let outcome = outcome.with_shared_permit(super::limiter::issue_shared_tile_permit());
    context.send_back(outcome).map_err(ProcedureError::Send)
}

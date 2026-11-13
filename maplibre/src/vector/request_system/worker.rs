//! Source fetching and completion of vector worker requests.

use std::{collections::HashSet, sync::Arc};

use super::group_tile::fetch_group_tile;

mod provided;
use crate::{
    coords::WorldTileCoords,
    environment::OffscreenKernel,
    io::{
        apc::{AsyncProcedureFuture, AttemptContext, Context, Input, ProcedureError, SendError},
        cpu_work::run_cpu_bound_replying,
        source_client::{HttpClient, SourceClient},
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    sdf::assets::{load_symbol_assets_awaiting_from_tile, SymbolAssetConfig, SymbolAtlas},
    style::{layer::StyleLayer, Style},
    vector::{
        process_vector::process_decoded_vector_tile_with_assets,
        transferables::{LayerMissing, TileTessellated, VectorTransferables},
        ProcessVectorContext, ProcessVectorError, VectorTileRequest,
    },
};

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
        let mut groups = source_layer_groups(&style, TileKind::Vector)
            .into_iter()
            .filter(|group| {
                only_sources.as_ref().is_none_or(|only| {
                    group
                        .source_name
                        .as_ref()
                        .is_some_and(|name| only.contains(name))
                })
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
            let tile = match fetch_group_tile(&client, coords, &group, refetch).await {
                Ok(tile) => tile,
                // A source without a tile there has nothing to draw, as GL JS loads a vector
                // tile that answered 404 empty: its layers are empty, and the tile's other
                // sources are drawn as usual.
                Err(error) if error.is_not_found() => {
                    tracing::debug!(%coords, source = ?group.source_name, "no vector tile at the source; its layers are empty");
                    source_empty::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                    continue;
                }
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
                SourcePass {
                    last_source: groups.peek().is_none() && !failed,
                    overscaled_zoom,
                    pixel_ratio,
                },
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
    tile: Arc<geozero::mvt::Tile>,
    coords: WorldTileCoords,
    style: &Style,
    client: &SourceClient<H>,
    group: &SourceLayerGroup,
    context: C,
    pass: SourcePass,
) -> Result<Option<provided::Symbols>, ProcessVectorError> {
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
    let base_indexed = !base.is_empty();
    let request = VectorTileRequest {
        coords,
        layers: base,
        projection: projection.clone(),
        overscaled_zoom,
    };
    tessellate::<T, C>(
        context.clone(),
        (last_source, true),
        (tile.clone(), request),
        source_state.clone(),
        Arc::new(SymbolAtlas::default()),
    )
    .await?;
    if symbols.is_empty() {
        return Ok(None);
    }
    // The assets are asked for by the very layers the symbols are laid out with: this source's,
    // as its tile is cut.
    let assets = load_symbol_assets_awaiting_from_tile(
        client,
        SymbolAssetConfig {
            pixel_ratio,
            ..SymbolAssetConfig::of(style)
        },
        &symbols,
        &tile,
        f64::from(overscaled_zoom.max(u8::from(coords.z))),
    )
    .await
    .map_err(ProcessVectorError::SymbolAssets)?;
    let request = VectorTileRequest {
        coords,
        layers: symbols,
        projection,
        overscaled_zoom,
    };
    // Only a source still waiting for images keeps its tile and layers to lay out again.
    let again = (!assets.awaiting.is_empty()).then(|| (tile.clone(), request.clone()));
    tessellate::<T, C>(
        context,
        (last_source, !base_indexed),
        (tile, request),
        source_state,
        assets.atlas,
    )
    .await?;
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
    (last_source, index_geometry): (bool, bool),
    (tile, request): (Arc<geozero::mvt::Tile>, VectorTileRequest),
    (promote_id, feature_state): (
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
    atlas: Arc<SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    run_cpu_bound_replying(context, move |context| {
        let mut processor = source_processor::<T, _>(context, last_source)
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
    last_source: bool,
) -> ProcessVectorContext<T, C> {
    let processor = ProcessVectorContext::new(context).with_pending_symbols();
    // Earlier sources cannot replace an ancestor before the other sources' base layers arrive.
    if last_source {
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
    context
        .send_back(TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt,
            disposition: if retry {
                RequestDisposition::Retry
            } else {
                RequestDisposition::Complete
            },
        })
        .map_err(ProcedureError::Send)
}

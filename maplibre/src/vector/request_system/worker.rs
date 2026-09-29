//! Source fetching and completion of vector worker requests.

use std::{collections::HashSet, sync::Arc};

use crate::{
    coords::WorldTileCoords,
    environment::OffscreenKernel,
    io::{
        apc::{AsyncProcedureFuture, AttemptContext, Context, Input, ProcedureError, SendError},
        source_client::{HttpClient, SourceClient},
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    sdf::assets::{load_symbol_assets, SymbolAtlas},
    style::{layer::StyleLayer, Style},
    vector::{
        process_vector::process_vector_tile_with_assets,
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
        let (coords, style, attempt) = input.into_tile_request();
        let context = AttemptContext::new(context, attempt);
        let client = kernel.source_client();
        let mut groups = source_layer_groups(&style, TileKind::Vector)
            .into_iter()
            .peekable();
        let mut failed = false;
        let mut retry = false;
        while let Some(group) = groups.next() {
            let data = match client.fetch(&coords, &group.source).await {
                Ok(data) => data,
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
                &data,
                coords,
                &style,
                &client,
                &group,
                context.clone(),
                groups.peek().is_none() && !failed,
            )
            .await
            {
                Ok(()) => {}
                Err(ProcessVectorError::SendError(source)) => {
                    return Err(ProcedureError::Send(source))
                }
                Err(error @ ProcessVectorError::Decoding { .. }) => {
                    tracing::warn!(%coords, source = ?group.source_name, %error, "invalid vector tile");
                    failed = true;
                    source_failed::<T, _>(coords, &group.layers, &context)
                        .map_err(ProcedureError::Send)?;
                }
            }
        }
        finish::<T, _>(&context, coords, attempt, failed, retry)
    })
}

fn source_failed<T: VectorTransferables, C: Context>(
    coords: WorldTileCoords,
    layers: &[StyleLayer],
    context: &C,
) -> Result<(), SendError> {
    for layer in layers {
        if let Some(source_layer) = &layer.source_layer {
            context.send_back(T::LayerMissing::build_from(coords, source_layer.clone()))?;
        }
    }
    context.send_back(T::TileTessellated::build_failed(coords, true))
}

async fn process_source<T: VectorTransferables, C: Context + Clone, H: HttpClient>(
    data: &[u8],
    coords: WorldTileCoords,
    style: &Style,
    client: &SourceClient<H>,
    group: &SourceLayerGroup,
    context: C,
    last_source: bool,
) -> Result<(), ProcessVectorError> {
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
    {
        let mut processor = source_processor::<T, C>(context.clone(), last_source);
        process_vector_tile_with_assets(
            data,
            VectorTileRequest {
                coords,
                layers: base,
                projection: projection.clone(),
            },
            &mut processor,
            Arc::new(SymbolAtlas::default()),
        )?;
    }
    if symbols.is_empty() {
        return Ok(());
    }
    let atlas = load_symbol_assets(client, style, data, f64::from(u8::from(coords.z))).await;
    let mut processor = source_processor::<T, C>(context, last_source);
    process_vector_tile_with_assets(
        data,
        VectorTileRequest {
            coords,
            layers: symbols,
            projection,
        },
        &mut processor,
        atlas,
    )
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

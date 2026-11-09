//! Fetches the sources of one raster tile on a worker and sends each back as it lands.

use futures::{stream::FuturesUnordered, StreamExt};

use crate::{
    coords::WorldTileCoords,
    environment::OffscreenKernel,
    io::{
        apc::{AsyncProcedureFuture, AttemptContext, Context, Input, ProcedureError},
        cpu_work::run_cpu_bound_replying,
        source_client::{HttpClient, SourceClient},
        source_type::SourceType,
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    raster::{
        process_raster::{
            process_raster_tile, ProcessRasterContext, ProcessRasterError, RasterTileRequest,
        },
        transferables::{LayerRasterMissing, RasterTransferables},
        RasterSourceId,
    },
};

/// Fetches every raster source of the tile at once, so a tile costs its slowest source's
/// round trip rather than the sum of them, and reports the outcome once all have landed.
pub fn fetch_raster_apc<K: OffscreenKernel, T: RasterTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> AsyncProcedureFuture {
    Box::pin(async move {
        let Input {
            coords,
            style,
            attempt,
            ..
        } = input;
        let context = AttemptContext::new(context, attempt);
        let client = kernel.source_client();
        let mut fetches: FuturesUnordered<_> = source_layer_groups(&style, TileKind::Raster)
            .into_iter()
            .map(|group| fetch_source::<T, _, _>(&client, group, coords, context.clone()))
            .collect();
        let mut retry = false;
        while let Some(fetched) = fetches.next().await {
            retry |= fetched?;
        }
        context
            .send_back(TileRequestOutcome {
                coords,
                kind: RequestKind::Raster,
                attempt: Some(attempt),
                disposition: if retry {
                    RequestDisposition::Retry
                } else {
                    RequestDisposition::Complete
                },
            })
            .map_err(ProcedureError::Send)
    })
}

/// Fetches one source of the tile and sends back its pixels, or that it has none; returns
/// whether a failed fetch is worth retrying.
async fn fetch_source<T: RasterTransferables, HC: HttpClient, C: Context + Clone + Send>(
    client: &SourceClient<HC>,
    group: SourceLayerGroup,
    coords: WorldTileCoords,
    context: C,
) -> Result<bool, ProcedureError> {
    let mut retry = false;
    if let SourceType::Image(image) = &group.source {
        return send_image_tile::<T, _, _>(
            (client, &image.source),
            (coords, RasterSourceId::new(group.source_name.clone())),
            context,
        )
        .await;
    }
    if group.source.format(&coords).is_some() {
        use crate::sdf::assets::tiles::CachedTileFailure;
        match client
            .assets()
            .load_raster_tile(client, &group.source, &coords)
            .await
        {
            Ok(rgba) => {
                let source = RasterSourceId::new(group.source_name.clone());
                // The shared image is copied into the reply, which is a full pass over
                // its pixels.
                let delivered = run_cpu_bound_replying(context.clone(), move |context| {
                    ProcessRasterContext::<T, _>::new(context).finish_decoded(
                        &coords,
                        source,
                        (*rgba).clone(),
                    )
                })
                .await;
                match delivered {
                    Ok(Err(ProcessRasterError::Send(source))) => {
                        return Err(ProcedureError::Send(source))
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%coords, source = ?group.source_name, %error, "raster tile not delivered");
                        context
                            .send_back(T::LayerRasterMissing::build_from(
                                coords,
                                RasterSourceId::new(group.source_name.clone()),
                            ))
                            .map_err(ProcedureError::Send)?;
                    }
                }
            }
            Err(CachedTileFailure::Decode(error)) => {
                tracing::warn!(%coords, source = ?group.source_name, %error, "invalid raster tile");
                context
                    .send_back(T::LayerRasterMissing::build_from(
                        coords,
                        RasterSourceId::new(group.source_name.clone()),
                    ))
                    .map_err(ProcedureError::Send)?;
            }
            Err(CachedTileFailure::NotFound) => {
                tracing::debug!(
                    %coords,
                    source = ?group.source_name,
                    "no raster tile at the source; the layer is empty"
                );
                context
                    .send_back(<T as RasterTransferables>::LayerRasterMissing::build_from(
                        coords,
                        RasterSourceId::new(group.source_name.clone()),
                    ))
                    .map_err(ProcedureError::Send)?;
            }
            Err(CachedTileFailure::Transport { retryable, message }) => {
                retry |= retryable;
                if retryable {
                    tracing::warn!(
                        %coords,
                        source = ?group.source_name,
                        error = %message,
                        "raster tile fetch failed"
                    );
                } else {
                    tracing::error!(
                        %coords,
                        source = ?group.source_name,
                        error = %message,
                        "raster tile fetch failed"
                    );
                }
                context
                    .send_back(<T as RasterTransferables>::LayerRasterMissing::build_from(
                        coords,
                        RasterSourceId::new(group.source_name.clone()),
                    ))
                    .map_err(ProcedureError::Send)?;
            }
        }
        return Ok(retry);
    }

    let data = match client.fetch(&coords, &group.source).await {
        Ok(data) => data,
        Err(error) => {
            let retryable = error.is_retryable();
            retry |= retryable;
            if error.is_not_found() {
                tracing::debug!(
                    %coords,
                    source = ?group.source_name,
                    "no raster tile at the source; the layer is empty"
                );
            } else if retryable {
                tracing::warn!(
                    %coords,
                    source = ?group.source_name,
                    error = %error.describe(),
                    "raster tile fetch failed"
                );
            } else {
                tracing::error!(
                    %coords,
                    source = ?group.source_name,
                    error = %error.describe(),
                    "raster tile fetch failed"
                );
            }

            context
                .send_back(<T as RasterTransferables>::LayerRasterMissing::build_from(
                    coords,
                    RasterSourceId::new(group.source_name.clone()),
                ))
                .map_err(ProcedureError::Send)?;
            return Ok(retry);
        }
    };
    let request = RasterTileRequest {
        coords,
        source: RasterSourceId::new(group.source_name.clone()),
    };
    let processed = run_cpu_bound_replying(context.clone(), move |context| {
        let mut process_context = ProcessRasterContext::<T, _>::new(context);
        process_raster_tile(&data, request, &mut process_context)
    })
    .await
    .map_err(ProcessRasterError::CpuWork)
    .and_then(|processed| processed);
    match processed {
        Ok(()) => {}
        Err(ProcessRasterError::Send(source)) => return Err(ProcedureError::Send(source)),
        Err(error @ (ProcessRasterError::Decoding { .. } | ProcessRasterError::CpuWork(_))) => {
            tracing::warn!(%coords, source = ?group.source_name, %error, "invalid raster tile");
            context
                .send_back(T::LayerRasterMissing::build_from(
                    coords,
                    RasterSourceId::new(group.source_name.clone()),
                ))
                .map_err(ProcedureError::Send)?;
        }
    }
    Ok(retry)
}

/// Sends the tile of an image source back, empty where the picture does not reach it, and
/// returns whether a failed fetch is worth retrying.
async fn send_image_tile<
    T: RasterTransferables,
    HC: crate::io::source_client::HttpClient,
    C: Context,
>(
    (client, image): (
        &crate::io::source_client::SourceClient<HC>,
        &crate::style::source::ImageSource,
    ),
    (coords, source): (WorldTileCoords, RasterSourceId),
    context: C,
) -> Result<bool, ProcedureError> {
    use crate::raster::transferables::LayerRaster;
    match crate::raster::image_source::load_tile(client, image, coords).await {
        Ok(tile) => {
            // A tile the picture does not reach has loaded too; it is simply empty.
            let tile = tile.unwrap_or_else(|| image::RgbaImage::new(1, 1));
            context
                .send_back(T::LayerRaster::build_from(coords, source, tile))
                .map_err(ProcedureError::Send)?;
            Ok(false)
        }
        Err(failure) => {
            context
                .send_back(T::LayerRasterMissing::build_from(coords, source))
                .map_err(ProcedureError::Send)?;
            Ok(failure.is_retryable())
        }
    }
}

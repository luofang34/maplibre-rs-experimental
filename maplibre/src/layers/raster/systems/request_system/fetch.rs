//! Fetches the sources of one raster tile on a worker and sends each back as it lands.

#[cfg(not(target_arch = "wasm32"))]
use crate::raster::systems::process_raster::process_raster_tile;
use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{
            run_cpu_bound_replying, AsyncProcedureFuture, AttemptContext, Context, Input,
            IntoMessage, OffscreenKernel, ProcedureError,
        },
        source_client::{
            limiter::{SharedTileClaim, SharedTileSlot},
            HttpClient, SourceClient,
        },
        source_type::SourceType,
        tile_retry::{RequestDisposition, RequestKind, TileRequestOutcome},
        tile_sources::{source_layer_groups, SourceLayerGroup, TileKind},
    },
    raster::{
        systems::process_raster::{ProcessRasterContext, ProcessRasterError, RasterTileRequest},
        transferables::{LayerRaster, LayerRasterMissing, RasterTransferables},
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
            only_sources,
            ..
        } = input;
        let context = AttemptContext::new(context, attempt);
        let client = kernel.source_client();
        let claim = SharedTileClaim::default();
        let fetches = source_layer_groups(&style, TileKind::Raster)
            .into_iter()
            .filter(|group| {
                !crate::raster::media_source::is_media_source(&style, group.source_name.as_deref())
                    && only_sources.as_ref().is_none_or(|only| {
                        group
                            .source_name
                            .as_deref()
                            .is_some_and(|name| only.iter().any(|allowed| allowed == name))
                    })
            })
            .map(|group| {
                let dem_unpack = group
                    .source_name
                    .as_deref()
                    .and_then(|name| style.sources.get(name))
                    .and_then(|src| match src {
                        crate::style::source::Source::RasterDem(dem) => Some(dem.unpack_vector()),
                        _ => None,
                    });
                fetch_source::<T, _, _>(&client, group, dem_unpack, coords, &claim, context.clone())
            });
        let outcomes = crate::util::async_join::try_join_all_concurrent(fetches.collect()).await?;
        let mut retry = false;
        let mut shared_slot = SharedTileSlot::default();
        for (source_retry, slot) in outcomes {
            retry |= source_retry;
            shared_slot.merge(slot);
        }
        let outcome = IntoMessage::into(TileRequestOutcome {
            coords,
            kind: RequestKind::Raster,
            attempt: Some(attempt),
            disposition: if retry {
                RequestDisposition::Retry
            } else {
                RequestDisposition::Complete
            },
        });
        context
            .send_back(outcome.with_shared_slot(shared_slot))
            .map_err(ProcedureError::Send)
    })
}

/// Fetches one source of the tile and sends back its pixels, or that it has none; returns
/// whether a failed fetch is worth retrying.
#[inline(never)]
async fn fetch_source<T: RasterTransferables, HC: HttpClient, C: Context + Clone + Send>(
    client: &SourceClient<HC>,
    group: SourceLayerGroup,
    dem_unpack: Option<[f64; 4]>,
    coords: WorldTileCoords,
    claim: &SharedTileClaim,
    context: C,
) -> Result<(bool, SharedTileSlot), ProcedureError> {
    let mut retry = false;
    if let SourceType::Image(image) = &group.source {
        let retry = send_image_tile::<T, _, _>(
            (client, &image.source),
            (coords, RasterSourceId::new(group.source_name.clone())),
            context,
        )
        .await?;
        return Ok((retry, SharedTileSlot::default()));
    }
    if group.has_nothing_at(coords)
        || group
            .max_zoom
            .is_some_and(|max_zoom| u8::from(coords.z) > max_zoom)
    {
        context
            .send_back(<T as RasterTransferables>::LayerRasterMissing::build_from(
                coords,
                RasterSourceId::new(group.source_name.clone()),
            ))
            .map_err(ProcedureError::Send)?;
        return Ok((false, SharedTileSlot::default()));
    }
    if let Some(unpack) = dem_unpack {
        if group.source.format(&coords).is_some() {
            return fetch_dem_source::<T, _, _>(client, group, unpack, coords, claim, context)
                .await;
        }
    }
    if group.source.format(&coords).is_some() {
        use crate::io::source_client::tile_cache::CachedTileFailure;
        match super::tile_cache::load_raster_tile(
            client.assets(),
            client,
            &group.source,
            &coords,
            claim,
        )
        .await
        {
            Ok((rgba, slot)) => {
                let source = RasterSourceId::new(group.source_name.clone());
                let owned_rgba =
                    std::sync::Arc::try_unwrap(rgba).unwrap_or_else(|arc| (*arc).clone());
                let delivered = run_cpu_bound_replying(context.clone(), move |context| {
                    ProcessRasterContext::<T, _>::new(context)
                        .finish_decoded(&coords, source, owned_rgba)
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
                return Ok((retry, slot));
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
        return Ok((retry, SharedTileSlot::default()));
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
            return Ok((retry, SharedTileSlot::default()));
        }
    };
    let request = RasterTileRequest {
        coords,
        source: RasterSourceId::new(group.source_name.clone()),
    };
    if data.is_empty() {
        context
            .send_back(T::LayerRaster::build_from(
                request.coords,
                request.source,
                image::RgbaImage::new(0, 0),
            ))
            .map_err(ProcedureError::Send)?;
        return Ok((retry, SharedTileSlot::default()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let processed = run_cpu_bound_replying(context.clone(), move |context| {
        let mut process_context = ProcessRasterContext::<T, _>::new(context);
        process_raster_tile(&data, request, &mut process_context)
    })
    .await
    .map_err(ProcessRasterError::CpuWork)
    .and_then(|processed| processed);
    #[cfg(target_arch = "wasm32")]
    let (processed, fallback_slot) = {
        let slot = claim.acquire_once().await;
        #[cfg(target_feature = "atomics")]
        let _heavy = crate::io::source_client::limiter::acquire_heavy_tile_slot().await;
        let res = match crate::platform::decode_rgba_bytes(&data).await {
            Ok(rgba) => run_cpu_bound_replying(context.clone(), move |context| {
                ProcessRasterContext::<T, _>::new(context).finish_decoded(
                    &request.coords,
                    request.source,
                    rgba,
                )
            })
            .await
            .map_err(ProcessRasterError::CpuWork)
            .and_then(|processed| processed),
            Err(reason) => Err(ProcessRasterError::Decoding {
                source: image::ImageError::Decoding(image::error::DecodingError::new(
                    image::error::ImageFormatHint::Unknown,
                    reason,
                )),
            }),
        };
        (res, slot)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let fallback_slot = SharedTileSlot::default();
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
    Ok((retry, fallback_slot))
}

/// Fetches, PNG-decodes, and despikes a `raster-dem` source tile on a worker through the shared
/// [`AssetCache`](crate::io::source_client::AssetCache), sharing the decoded [`DemTile`](crate::terrain::DemTile)
/// with 3D terrain when both reference the same source.
async fn fetch_dem_source<T: RasterTransferables, HC: HttpClient, C: Context>(
    client: &SourceClient<HC>,
    group: SourceLayerGroup,
    unpack: [f64; 4],
    coords: WorldTileCoords,
    claim: &SharedTileClaim,
    context: C,
) -> Result<(bool, SharedTileSlot), ProcedureError> {
    use crate::{
        io::source_client::tile_cache::CachedTileFailure, projection::body::Body,
        raster::transferables::LayerRaster,
    };
    let source = RasterSourceId::new(group.source_name.clone());
    let circumference = Body::EARTH.circumference_meters();
    match crate::terrain::dem::cache::load_dem_tile(
        client.assets(),
        client,
        &group.source,
        &coords,
        unpack,
        circumference,
        claim,
    )
    .await
    {
        Ok((entry, slot)) => {
            context
                .send_back(T::LayerRaster::build_from_dem(
                    coords,
                    source,
                    std::sync::Arc::clone(&entry.tile),
                    entry.circumference_meters,
                    std::sync::Arc::clone(&entry.image),
                ))
                .map_err(ProcedureError::Send)?;
            Ok((false, slot))
        }
        Err(CachedTileFailure::Decode(error)) => {
            tracing::warn!(%coords, source = ?group.source_name, %error, "invalid DEM raster tile");
            context
                .send_back(T::LayerRasterMissing::build_from(coords, source))
                .map_err(ProcedureError::Send)?;
            Ok((false, SharedTileSlot::default()))
        }
        Err(CachedTileFailure::NotFound) => {
            tracing::debug!(%coords, source = %group.source_name.as_deref().unwrap_or_default(), "no DEM raster tile at the source");
            context
                .send_back(T::LayerRasterMissing::build_from(coords, source))
                .map_err(ProcedureError::Send)?;
            Ok((false, SharedTileSlot::default()))
        }
        Err(CachedTileFailure::Transport { retryable, message }) => {
            if retryable {
                tracing::warn!(%coords, source = ?group.source_name, error = %message, "DEM raster tile fetch failed");
            } else {
                tracing::error!(%coords, source = ?group.source_name, error = %message, "DEM raster tile fetch failed");
            }
            context
                .send_back(T::LayerRasterMissing::build_from(coords, source))
                .map_err(ProcedureError::Send)?;
            Ok((retryable, SharedTileSlot::default()))
        }
    }
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

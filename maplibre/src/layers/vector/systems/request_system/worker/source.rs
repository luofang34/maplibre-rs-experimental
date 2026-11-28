use super::*;

pub(super) struct SourceSlots<'a> {
    pub(super) heavy: &'a mut Option<super::super::limiter::HeavyTileSlot>,
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub(super) shared: &'a mut Option<super::super::limiter::SharedTilePermit>,
}

pub(super) async fn process_source<
    T: VectorTransferables,
    C: Context + Clone + Send,
    H: HttpClient,
>(
    group_tile: GroupTile,
    coords: WorldTileCoords,
    style: &Style,
    client: &SourceClient<H>,
    group: &SourceLayerGroup,
    context: C,
    (slots, pass): (SourceSlots<'_>, SourcePass),
) -> Result<Option<provided::Symbols>, ProcessVectorError> {
    let GroupTile {
        mut tile,
        mvt_bytes,
        mvt_compressed,
        mvt_rescale,
    } = group_tile;
    let projection = style
        .projection
        .as_ref()
        .map_or_else(Default::default, |spec| spec.projection_type.clone());
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
        emit_geometry_index::<T, _>(
            &context,
            coords,
            group,
            (mvt_bytes, mvt_compressed, mvt_rescale),
            &source_state,
        )?;
    }
    let request = make_tile_request(coords, base, projection.clone(), pass.overscaled_zoom);
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
        drop(slots.heavy.take());
        return Ok(None);
    }
    let sym_request = make_tile_request(coords, symbols, projection, pass.overscaled_zoom);
    process_source_with_symbols::<T, C, H>(
        SourceSymbolsArgs {
            tile: &mut tile,
            coords,
            style,
            client,
            context,
            base_request: request,
            sym_request,
            source_state,
            has_dem_source,
            pass,
        },
        slots,
    )
    .await
}

#[expect(
    clippy::mutable_key_type,
    reason = "StyleLayer Hash and Eq compare only immutable layer IDs"
)]
fn make_tile_request(
    coords: WorldTileCoords,
    layers: HashSet<StyleLayer>,
    projection: crate::projection::ProjectionType,
    overscaled_zoom: u8,
) -> VectorTileRequest {
    VectorTileRequest {
        coords,
        layers,
        projection,
        overscaled_zoom,
    }
}

fn emit_geometry_index<T: VectorTransferables, C: Context>(
    context: &C,
    coords: WorldTileCoords,
    group: &SourceLayerGroup,
    (mvt_bytes, mvt_compressed, mvt_rescale): (
        Arc<[u8]>,
        bool,
        Option<(WorldTileCoords, WorldTileCoords)>,
    ),
    source_state: &(
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
) -> Result<(), ProcessVectorError> {
    crate::tcs::geometry_index::register_mvt_materializer(
        crate::io::vector_tile::index_mvt_bytes_rescaled,
    );
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
        .map_err(ProcessVectorError::SendError)
}

struct SourceSymbolsArgs<'a, C, H: HttpClient> {
    tile: &'a mut Arc<crate::vector::process_vector::DecodedVectorTile>,
    coords: WorldTileCoords,
    style: &'a Style,
    client: &'a SourceClient<H>,
    context: C,
    base_request: VectorTileRequest,
    sym_request: VectorTileRequest,
    source_state: (
        Option<crate::style::source::PromoteId>,
        Option<crate::style::feature_state::SourceFeatureState>,
    ),
    has_dem_source: bool,
    pass: SourcePass,
}

async fn process_source_with_symbols<
    T: VectorTransferables,
    C: Context + Clone + Send,
    H: HttpClient,
>(
    args: SourceSymbolsArgs<'_, C, H>,
    slots: SourceSlots<'_>,
) -> Result<Option<provided::Symbols>, ProcessVectorError> {
    let zoom = f64::from(args.pass.overscaled_zoom.max(u8::from(args.coords.z)));
    let sym_requests =
        symbol_requests_from_tile(&args.sym_request.layers, args.tile, zoom, Some(args.style));
    let asset_config = SymbolAssetConfig {
        pixel_ratio: args.pass.pixel_ratio,
        ..SymbolAssetConfig::of(args.style)
    };
    let mut assets_fut = std::pin::pin!(fetch_symbol_asset_inputs(
        args.client,
        asset_config,
        sym_requests
    ));
    let mut pre_assets = None;
    poll_ready_once(assets_fut.as_mut(), &mut pre_assets).await;
    tessellate::<T, C>(
        args.context.clone(),
        (false, false, args.has_dem_source),
        (args.tile.clone(), args.base_request),
        args.source_state.clone(),
        Arc::new(SymbolAtlas::default()),
    )
    .await?;
    retain_symbol_mvt_layers(args.tile, &args.sym_request.layers);
    if pre_assets.is_none() {
        poll_ready_once(assets_fut.as_mut(), &mut pre_assets).await;
    }
    let fetched_inputs = match pre_assets {
        Some(res) => res,
        None => {
            if args.pass.last_source {
                args.context
                    .send_back(T::TileTessellated::build_partial(args.coords))
                    .map_err(ProcessVectorError::SendError)?;
            }
            // Release HeavyTileSlot and SharedTilePermit strictly BEFORE awaiting remote HTTP I/O.
            drop(slots.heavy.take());
            #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
            drop(slots.shared.take());
            let loaded = assets_fut.await;
            if loaded.is_ok() {
                // Re-acquire SharedTilePermit BEFORE HeavyTileSlot to match initial lock order.
                #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
                if slots.shared.is_none() {
                    *slots.shared = Some(super::super::limiter::acquire_shared_tile_permit().await);
                }
                *slots.heavy = Some(acquire_heavy_tile_slot().await);
            }
            loaded
        }
    }
    .map_err(ProcessVectorError::SymbolAssets)?;
    let assets = pack_symbol_assets(args.client, asset_config, fetched_inputs);
    // Only a source still waiting for images keeps its tile and layers to lay out again.
    let again =
        (!assets.awaiting.is_empty()).then(|| (args.tile.clone(), args.sym_request.clone()));
    tessellate::<T, C>(
        args.context,
        (false, false, args.has_dem_source),
        (args.tile.clone(), args.sym_request),
        args.source_state,
        assets.atlas,
    )
    .await?;
    drop(slots.heavy.take());
    Ok(Some(provided::Symbols {
        again,
        pixel_ratio: args.pass.pixel_ratio,
        provided: assets.provided,
        awaiting: assets.awaiting,
    }))
}

async fn poll_ready_once<F, T>(mut fut: std::pin::Pin<&mut F>, out: &mut Option<T>)
where
    F: std::future::Future<Output = T>,
{
    std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(res) = std::future::Future::poll(fut.as_mut(), cx) {
            *out = Some(res);
        }
        std::task::Poll::Ready(())
    })
    .await;
}

#[expect(
    clippy::mutable_key_type,
    reason = "StyleLayer Hash and Eq compare only immutable layer IDs"
)]
fn retain_symbol_mvt_layers(
    tile: &mut Arc<crate::vector::process_vector::DecodedVectorTile>,
    layers: &HashSet<StyleLayer>,
) {
    if let Some(crate::vector::process_vector::DecodedVectorTile::Mvt(inner)) = Arc::get_mut(tile) {
        inner.layers.retain(|mvt| {
            layers
                .iter()
                .any(|sl| sl.source_layer.as_deref() == Some(mvt.name.as_str()))
        });
    }
}

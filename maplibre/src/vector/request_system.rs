//! Requests tiles which are currently in view

use std::{borrow::Cow, collections::HashSet, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::{Environment, OffscreenKernel},
    io::{
        apc::{AsyncProcedureCall, AsyncProcedureFuture, Context, Input, ProcedureError},
        tile_backpressure::request_budget,
        tile_sources::{
            clamp_to_max_zoom, source_layer_groups, source_max_zoom, source_min_zoom, TileKind,
        },
    },
    kernel::Kernel,
    render::{
        projection::view_region_for_projection, tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::ViewStatePadding,
    },
    sdf::SymbolLayersDataComponent,
    style::layer::StyleLayer,
    tcs::system::{System, SystemResult},
    vector::{
        process_vector::{
            process_vector_tile_with_assets, ProcessVectorContext, VectorTileRequest,
        },
        transferables::{LayerMissing, TileTessellated, VectorTransferables},
        VectorLayerBucketComponent,
    },
};

pub struct RequestSystem<E: Environment, T> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T> RequestSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: Default::default(),
        }
    }
}

impl<E: Environment, T: VectorTransferables> System for RequestSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "vector_request".into()
    }

    fn run(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        let view_region = view_region_for_projection(
            style,
            view_state,
            world,
            view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
            ViewStatePadding::Loose,
        )
        .map_err(|error| {
            tracing::error!(%error, "unable to select vector request tiles");
            crate::tcs::system::SystemError::Setup
        })?;

        // Tile arrivals, eviction and a settling eye can change the covering without motion.
        if let Some(view_region) = &view_region {
            let max_zoom = source_max_zoom(style, TileKind::Vector);
            let min_zoom = source_min_zoom(style, TileKind::Vector);
            let mut requested = HashSet::new();
            let mut budget = request_budget(world);
            if style.terrain.is_some() {
                // DEM requests run later in Extract and need slots to load the surface.
                budget = budget.saturating_sub(4);
            }

            for coords in view_region.iter() {
                // Above the source maximum zoom the ancestor tile is fetched once and the
                // view pattern scales it into every descendant in view.
                if min_zoom.is_some_and(|min_zoom| u8::from(coords.z) < min_zoom) {
                    continue;
                }
                let coords = clamp_to_max_zoom(coords, max_zoom);
                if coords.build_quad_key().is_none() || !requested.insert(coords) {
                    continue;
                }

                // TODO: Make tessellation depend on style? So maybe we need to request even if it exists
                if world
                    .tiles
                    .query::<&VectorLayerBucketComponent>(coords)
                    .is_some()
                {
                    continue;
                }
                // The rest wait for a later frame, once tiles in flight have landed.
                if budget == 0 {
                    break;
                }
                budget -= 1;

                self.request(coords, style, world)?;
            }
        }
        Ok(())
    }
}

pub fn fetch_vector_apc<K: OffscreenKernel, T: VectorTransferables, C: Context + Clone + Send>(
    input: Input,
    context: C,
    kernel: K,
) -> AsyncProcedureFuture {
    Box::pin(async move {
        let Input::TileRequest { coords, style } = input else {
            return Err(ProcedureError::IncompatibleInput);
        };

        let client = kernel.source_client();
        let projection = style
            .projection
            .as_ref()
            .map_or_else(Default::default, |specification| {
                specification.projection_type.clone()
            });

        for group in source_layer_groups(&style, TileKind::Vector) {
            let requested_layers: HashSet<StyleLayer> = group.layers.iter().cloned().collect();
            if requested_layers.is_empty() {
                continue;
            }
            let data = match client.fetch(&coords, &group.source).await {
                Ok(data) => data,
                Err(error) => {
                    tracing::warn!(%coords,source=?group.source_name,error=%error.describe(),"vector tile unavailable");
                    for layer in requested_layers {
                        context
                            .send_back(T::LayerMissing::build_from(coords, layer.id))
                            .map_err(ProcedureError::Send)?;
                    }
                    continue;
                }
            };
            let (symbols, base): (HashSet<_>, HashSet<_>) =
                requested_layers.into_iter().partition(|layer| {
                    matches!(
                        layer.paint,
                        Some(crate::style::layer::LayerPaint::Symbol(_))
                    )
                });
            {
                let mut processor =
                    ProcessVectorContext::<T, C>::new(context.clone()).with_pending_symbols();
                process_vector_tile_with_assets(
                    &data,
                    VectorTileRequest {
                        coords,
                        layers: base,
                        projection: projection.clone(),
                    },
                    &mut processor,
                    std::sync::Arc::new(crate::sdf::assets::SymbolAtlas::default()),
                )
                .map_err(|error| ProcedureError::Execution(Box::new(error)))?;
            }
            if symbols.is_empty() {
                continue;
            }
            let atlas = crate::sdf::assets::load_symbol_assets(
                &client,
                &style,
                &data,
                f64::from(u8::from(coords.z)),
            )
            .await;
            let mut processor =
                ProcessVectorContext::<T, C>::new(context.clone()).with_pending_symbols();
            process_vector_tile_with_assets(
                &data,
                VectorTileRequest {
                    coords,
                    layers: symbols,
                    projection: projection.clone(),
                },
                &mut processor,
                atlas,
            )
            .map_err(|error| ProcedureError::Execution(Box::new(error)))?;
        }

        context
            .send_back(T::TileTessellated::build_from(coords))
            .map_err(ProcedureError::Send)?;
        Ok(())
    })
}

impl<E: Environment, T: VectorTransferables> RequestSystem<E, T> {
    fn request(
        &self,
        coords: crate::coords::WorldTileCoords,
        style: &crate::style::Style,
        world: &mut crate::tcs::world::World,
    ) -> SystemResult {
        let Some(mut tile) = world.tiles.spawn_mut(coords) else {
            return Err(crate::tcs::system::SystemError::Setup);
        };
        tile.insert(VectorLayerBucketComponent::default())
            .insert(SymbolLayersDataComponent::default());
        tracing::debug!(%coords, "tile request started");
        self.kernel.apc().call(Input::TileRequest { coords, style: style.clone() },
            fetch_vector_apc::<E::OffscreenKernelEnvironment, T, <E::AsyncProcedureCall as AsyncProcedureCall<E::OffscreenKernelEnvironment>>::Context>)
            .map_err(|error| {
                tracing::error!(%coords, ?error, "unable to schedule tile request");
                crate::tcs::system::SystemError::Setup
            })
    }
}

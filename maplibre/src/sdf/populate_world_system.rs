use std::{borrow::Cow, marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    io::{
        apc::{apply_worker_messages, AsyncProcedureCall},
        tile_retry::{self, RequestKind},
    },
    kernel::Kernel,
    tcs::system::{System, SystemResult},
    vector::transferables::*,
};

pub struct PopulateWorldSystem<E: Environment, T> {
    kernel: Rc<Kernel<E>>,
    phantom_t: PhantomData<T>,
}

impl<E: Environment, T> PopulateWorldSystem<E, T> {
    pub fn new(kernel: &Rc<Kernel<E>>) -> Self {
        Self {
            kernel: kernel.clone(),
            phantom_t: Default::default(),
        }
    }
}

impl<E: Environment, T: VectorTransferables> System for PopulateWorldSystem<E, T> {
    fn name(&self) -> Cow<'static, str> {
        "sdf_populate_world_system".into()
    }

    fn run(
        &mut self,
        MapContext {
            world,
            style,
            renderer,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        let mut messages = self
            .kernel
            .apc()
            .receive(|message| {
                message.has_tag(T::SymbolLayerTessellated::message_tag())
                    || message.has_tag(super::provided::ProvidedImagesReport::message_tag())
            })
            .peekable();
        if messages.peek().is_none() || renderer.settings.symbol_pools.is_empty() {
            return Ok(());
        }
        let layer_by_id = style.cached_layer_index_by_id();
        let mut prop_interner =
            crate::vector::feature_paint::intern::TilePropertyInterner::default();
        let mut state_memo = crate::style::feature_state::FeatureStateArcMemo::default();
        let mut handled = false;
        apply_worker_messages(messages, |message| {
            handled = true;
            if message.has_tag(super::provided::ProvidedImagesReport::message_tag()) {
                let report =
                    message.into_transferable::<super::provided::ProvidedImagesReport>()?;
                super::provided::apply(world, *report);
            } else if message.has_tag(T::SymbolLayerTessellated::message_tag()) {
                let attempt = message.attempt();
                let message = message.into_transferable::<T::SymbolLayerTessellated>()?;
                let coords = message.coords();
                let mut bucket = message.to_bucket();
                if let Some(style_layer) =
                    tile_retry::accepts(world, coords, RequestKind::Vector, attempt)
                        .then(|| layer_by_id.get(bucket.style_layer_id.as_str()))
                        .flatten()
                        .and_then(|&idx| style.layers.get(idx))
                {
                    prop_interner.intern_symbol_bucket(&world.tiles, &mut bucket);
                    let retessellate =
                        crate::vector::arrival_sync::sync_arriving_symbol_bucket_for_layer(
                            style,
                            style_layer,
                            &mut bucket,
                            &mut state_memo,
                        );
                    crate::vector::content::accept_symbols(world, bucket);
                    if retessellate {
                        tile_retry::refresh_tiles(world, RequestKind::Vector, &[coords]);
                    }
                }
            }
            Ok(())
        })?;
        if handled {
            crate::vector::bump_arrival_epoch(world);
        }

        Ok(())
    }
}

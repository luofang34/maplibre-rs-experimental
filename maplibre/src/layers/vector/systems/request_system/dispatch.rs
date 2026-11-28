use std::{rc::Rc, sync::Arc};

use super::worker::fetch_vector_apc;
use crate::{
    environment::Environment,
    io::{
        apc::{AsyncProcedureCall, Input, ProcedureTag},
        frame_style::FrameStyle,
        tile_retry::{self, RequestKind},
    },
    kernel::Kernel,
    schedule::system::{SystemError, SystemResult},
    vector::{transferables::VectorTransferables, VectorLayerBucketComponent},
};

pub(super) fn request_tile<E: Environment, T: VectorTransferables>(
    kernel: &Rc<Kernel<E>>,
    coords: crate::coords::WorldTileCoords,
    style: &mut FrameStyle,
    world: &mut crate::tcs::world::World,
    (overscaled_zoom, pixel_ratio): (u8, f32),
    only_sources: Option<Vec<String>>,
    (vectors_disabled, symbols_disabled): (bool, bool),
) -> SystemResult {
    if world.tiles.spawn_mut(coords).is_none() {
        return Err(SystemError::InvalidTile { coords });
    }
    let refetch = tile_retry::due(world, coords, RequestKind::Vector);
    let attempt = tile_retry::next_attempt(world);
    let issued_for = only_sources.clone();
    let worker_style = style.filtered_worker_snapshot(vectors_disabled, symbols_disabled);
    // What the worker is asked to build: its style's non-symbol layers, for the refreshed
    // sources alone when the request is limited to them.
    let requested: Arc<[String]> = if only_sources.is_none() && !vectors_disabled {
        style.style().cached_vector_layer_ids()
    } else {
        worker_style
            .layers
            .iter()
            .filter(|layer| {
                layer.type_ != "symbol"
                    && crate::style::mutation::from_vector_tiles(layer)
                    && only_sources.as_ref().is_none_or(|sources| {
                        layer
                            .source
                            .as_ref()
                            .is_some_and(|source| sources.contains(source))
                    })
            })
            .map(|layer| layer.id.clone())
            .collect::<Vec<_>>()
            .into()
    };
    kernel
        .apc()
        .call_tagged(
            ProcedureTag::FetchVector,
            Input {
                coords,
                style: worker_style,
                attempt,
                overscaled_zoom,
                pixel_ratio,
                only_sources,
                refetch,
            },
            fetch_vector_apc::<E::OffscreenKernelEnvironment, T, _>,
        )
        .map_err(|source| SystemError::TileRequest {
            kind: "vector",
            coords,
            source,
        })?;
    crate::vector::content::begin(world, coords, requested);
    if let Some(tile) = world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(coords)
    {
        tile.overscaled_zoom = overscaled_zoom;
    }
    tile_retry::started(
        world,
        coords,
        RequestKind::Vector,
        attempt,
        issued_for.as_deref(),
    );
    if let Some(replaced) = crate::sdf::provided::restarted(world, coords, pixel_ratio) {
        kernel.apc().cancel(replaced);
    }
    tracing::debug!(%coords, "vector tile request accepted");
    Ok(())
}

/// Whether tiles are laid out for the zoom they are magnified to: layout and filter
/// expressions are evaluated at that zoom, and labels along a line space themselves by it.
pub fn depends_on_overscaling(style: &crate::style::Style) -> bool {
    style.layers.iter().any(|layer| layer.paint.is_some())
}

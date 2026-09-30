//! Completion flags shared between Web worker replies and tile coverage selection.

use flatbuffers::FlatBufferBuilder;
use maplibre::coords::WorldTileCoords;

use super::{
    FlatBufferTransferable, FlatTileTessellatedBuilder, FlatWorldTileCoords, WebMessageTag,
};

pub(super) fn tile_completion(
    coords: WorldTileCoords,
    pending: bool,
    failed: bool,
) -> FlatBufferTransferable {
    let mut inner_builder = FlatBufferBuilder::with_capacity(1024);
    let mut builder = FlatTileTessellatedBuilder::new(&mut inner_builder);

    builder.add_coords(&FlatWorldTileCoords::new(
        coords.x,
        coords.y,
        coords.z.into(),
    ));
    builder.add_pending_symbols(pending);
    builder.add_failed(failed);
    let root = builder.finish();
    inner_builder.finish(root, None);
    let (data, start) = inner_builder.collapse();
    FlatBufferTransferable {
        tag: WebMessageTag::TileTessellated,
        data,
        start,
    }
}

#[cfg(test)]
mod tests;

//! Completion flags shared between Web worker replies and tile coverage selection.

use maplibre::coords::WorldTileCoords;

use super::{FlatBufferTransferable, WebMessageTag};

pub(super) fn tile_completion(
    coords: WorldTileCoords,
    pending: bool,
    failed: bool,
) -> FlatBufferTransferable {
    let mut data = Vec::with_capacity(11);
    data.extend_from_slice(&coords.x.to_le_bytes());
    data.extend_from_slice(&coords.y.to_le_bytes());
    data.push(u8::from(coords.z));
    data.push(u8::from(pending));
    data.push(u8::from(failed));
    FlatBufferTransferable::from_vec(WebMessageTag::TileTessellated, data, 0)
}

#[cfg(test)]
mod tests;

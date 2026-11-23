//! Completion flags and missing-layer transferables shared between workers and the main thread.

use maplibre::{
    coords::WorldTileCoords,
    io::apc::MessageTag,
    vector::{LayerMissing, MissingVectorLayerBucket, TileTessellated},
};

#[cfg(any(test, feature = "renderer"))]
use super::UnpackedTiny;
use super::{read_coords_at, read_u32_le, FlatBufferTransferable, WebMessageTag};

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

pub(super) fn encode_named_coords(
    tag: WebMessageTag,
    coords: WorldTileCoords,
    name: Option<&str>,
) -> FlatBufferTransferable {
    let name_bytes = name.map(str::as_bytes);
    let name_len = name_bytes.map_or(0, <[u8]>::len);
    let mut data = Vec::with_capacity(16 + name_len);
    data.extend_from_slice(&coords.x.to_le_bytes());
    data.extend_from_slice(&coords.y.to_le_bytes());
    data.push(u8::from(coords.z));
    data.push(u8::from(name_bytes.is_some()));
    data.extend_from_slice(&[0, 0]);
    data.extend_from_slice(&(name_len as u32).to_le_bytes());
    if let Some(bytes) = name_bytes {
        data.extend_from_slice(bytes);
    }
    FlatBufferTransferable::from_vec(tag, data, 0)
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn parse_named_coords_strict(bytes: &[u8]) -> Option<(WorldTileCoords, Option<&str>)> {
    let coords = read_coords_at(bytes, 0)?;
    let has_name = *bytes.get(9)? != 0;
    if !has_name {
        return Some((coords, None));
    }
    let len = read_u32_le(bytes, 12)? as usize;
    let name = std::str::from_utf8(bytes.get(16..16 + len)?).ok()?;
    Some((coords, Some(name)))
}

pub(super) fn decode_named_coords(bytes: &[u8]) -> (WorldTileCoords, Option<&str>) {
    let Some(coords) = read_coords_at(bytes, 0) else {
        return (WorldTileCoords::default(), None);
    };
    let has_name = bytes.get(9).is_some_and(|&b| b != 0);
    if !has_name {
        return (coords, None);
    }
    let name = read_u32_le(bytes, 12)
        .and_then(|len| bytes.get(16..16 + len as usize))
        .and_then(|slice| std::str::from_utf8(slice).ok());
    (coords, name)
}

impl TileTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::TileTessellated
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        tile_completion(coords, false, false)
    }

    fn build_partial(coords: WorldTileCoords) -> Self {
        tile_completion(coords, true, false)
    }

    fn build_failed(coords: WorldTileCoords, pending_symbols: bool) -> Self {
        tile_completion(coords, pending_symbols, true)
    }

    fn failed(&self) -> bool {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Completion(_, _, failed)) = self.unpacked_tiny {
            return failed;
        }
        self.data().get(10).is_none_or(|&b| b != 0)
    }

    fn pending_symbols(&self) -> bool {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Completion(_, pending, _)) = self.unpacked_tiny {
            return pending;
        }
        self.data().get(9).is_some_and(|&b| b != 0)
    }

    fn coords(&self) -> WorldTileCoords {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Completion(coords, _, _)) = self.unpacked_tiny {
            return coords;
        }
        self.data()
            .get(..11)
            .and_then(|bytes| read_coords_at(bytes, 0))
            .unwrap_or_default()
    }
}

impl LayerMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerMissing
    }

    fn build_from(coords: WorldTileCoords, layer_name: String) -> Self {
        encode_named_coords(WebMessageTag::LayerMissing, coords, Some(&layer_name))
    }

    fn coords(&self) -> WorldTileCoords {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Named(coords, _)) = &self.unpacked_tiny {
            return *coords;
        }
        decode_named_coords(self.data()).0
    }

    fn layer_name(&self) -> &str {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Named(_, name)) = &self.unpacked_tiny {
            return name.as_deref().unwrap_or_default();
        }
        decode_named_coords(self.data()).1.unwrap_or_default()
    }

    fn to_bucket(self) -> MissingVectorLayerBucket {
        #[cfg(any(test, feature = "renderer"))]
        if let Some(UnpackedTiny::Named(coords, name)) = self.unpacked_tiny {
            return MissingVectorLayerBucket {
                source_layer: name.unwrap_or_default(),
                coords,
            };
        }
        let (coords, name) = decode_named_coords(self.data());
        MissingVectorLayerBucket {
            source_layer: name.unwrap_or_default().to_owned(),
            coords,
        }
    }
}

#[cfg(test)]
mod tests;

//! Source identities retained across raster worker binary transfers.

use std::sync::Arc;

use maplibre::{raster::SharedRasterDem, terrain::DemTile};

use super::*;

const RASTER_HEADER_BYTES: usize = 24;
const RASTER_DEM_MAGIC: u16 = 0x5244;
const RASTER_DEM_HEADER_BYTES: usize = 80;

fn is_raw_dem_frame(bytes: &[u8]) -> bool {
    bytes.len() >= RASTER_DEM_HEADER_BYTES
        && bytes.get(10..12) == Some(&RASTER_DEM_MAGIC.to_le_bytes())
}

struct RawRasterDemHeader {
    coords: WorldTileCoords,
    source_name: Option<String>,
    dim: u32,
    stride: u32,
    unpack: [f64; 4],
    min: f64,
    max: f64,
    circumference_meters: f64,
    px_offset: usize,
}

fn parse_raw_raster_dem_header(
    bytes: &[u8],
) -> Result<RawRasterDemHeader, AvailableRasterLayerData> {
    let empty = || AvailableRasterLayerData {
        coords: WorldTileCoords::default(),
        source: RasterSourceId::default(),
        image: RgbaImage::new(0, 0),
    };
    let Some(coords) = read_coords_at(bytes, 0) else {
        return Err(empty());
    };
    let has_name = bytes[9] != 0;
    let (Some(dim), Some(stride), Some(name_len)) = (
        read_u32_le(bytes, 12),
        read_u32_le(bytes, 16),
        read_u32_le(bytes, 20),
    ) else {
        return Err(empty());
    };
    let (Some(u0), Some(u1), Some(u2), Some(u3), Some(min), Some(max), Some(circumference_meters)) = (
        read_f64_le(bytes, 24),
        read_f64_le(bytes, 32),
        read_f64_le(bytes, 40),
        read_f64_le(bytes, 48),
        read_f64_le(bytes, 56),
        read_f64_le(bytes, 64),
        read_f64_le(bytes, 72),
    ) else {
        return Err(empty());
    };
    let Some(px_offset) = RASTER_DEM_HEADER_BYTES.checked_add(name_len as usize) else {
        return Err(empty());
    };
    let source_name = if has_name {
        bytes
            .get(RASTER_DEM_HEADER_BYTES..px_offset)
            .and_then(|slice| std::str::from_utf8(slice).ok())
            .map(str::to_owned)
    } else {
        None
    };
    if bytes.len() < px_offset {
        return Err(AvailableRasterLayerData {
            coords,
            source: RasterSourceId::new(source_name),
            image: RgbaImage::new(0, 0),
        });
    }
    Ok(RawRasterDemHeader {
        coords,
        source_name,
        dim,
        stride,
        unpack: [u0, u1, u2, u3],
        min,
        max,
        circumference_meters,
        px_offset,
    })
}

fn decode_rgba_layer(transferable: FlatBufferTransferable) -> AvailableRasterLayerData {
    let bytes = transferable.data();
    let empty = || AvailableRasterLayerData {
        coords: WorldTileCoords::default(),
        source: RasterSourceId::default(),
        image: RgbaImage::new(0, 0),
    };
    if bytes.len() < RASTER_HEADER_BYTES {
        return empty();
    }
    let Some(coords) = read_coords_at(bytes, 0) else {
        return empty();
    };
    let has_name = bytes[9] != 0;
    let (Some(width), Some(height), Some(name_len)) = (
        read_u32_le(bytes, 12),
        read_u32_le(bytes, 16),
        read_u32_le(bytes, 20),
    ) else {
        return empty();
    };
    let Some(px_offset) = RASTER_HEADER_BYTES.checked_add(name_len as usize) else {
        return empty();
    };
    let source_name = if has_name {
        bytes
            .get(RASTER_HEADER_BYTES..px_offset)
            .and_then(|slice| std::str::from_utf8(slice).ok())
            .map(str::to_owned)
    } else {
        None
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if expected != bytes.get(px_offset..).map(<[u8]>::len) {
        return AvailableRasterLayerData {
            coords,
            source: RasterSourceId::new(source_name),
            image: RgbaImage::new(0, 0),
        };
    }
    let pixels = transferable.into_tail_vec(px_offset);
    let image = RgbaImage::from_vec(width, height, pixels).unwrap_or_else(|| RgbaImage::new(0, 0));
    AvailableRasterLayerData {
        coords,
        source: RasterSourceId::new(source_name),
        image,
    }
}

fn decode_dem_frame(
    transferable: FlatBufferTransferable,
) -> Result<SharedRasterDem, AvailableRasterLayerData> {
    let hdr = parse_raw_raster_dem_header(transferable.data())?;
    let pixels = transferable.into_tail_vec(hdr.px_offset);
    match DemTile::from_raw_parts(hdr.dim, hdr.stride, pixels, hdr.unpack, hdr.min, hdr.max) {
        Ok(dem) => Ok(SharedRasterDem {
            coords: hdr.coords,
            source: RasterSourceId::new(hdr.source_name),
            tile: Arc::new(dem),
            circumference_meters: hdr.circumference_meters,
            fallback: Arc::new(RgbaImage::new(0, 0)),
        }),
        Err(_) => Err(AvailableRasterLayerData {
            coords: hdr.coords,
            source: RasterSourceId::new(hdr.source_name),
            image: RgbaImage::new(0, 0),
        }),
    }
}

impl LayerRaster for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerRaster
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId, image: RgbaImage) -> Self {
        let name_bytes = source.name().map(str::as_bytes);
        let name_len = name_bytes.map_or(0, <[u8]>::len);
        let width = image.width();
        let height = image.height();
        let pixels = image.into_vec();
        let mut data = Vec::with_capacity(RASTER_HEADER_BYTES + name_len + pixels.len());
        data.extend_from_slice(&coords.x.to_le_bytes());
        data.extend_from_slice(&coords.y.to_le_bytes());
        data.push(u8::from(coords.z));
        data.push(u8::from(name_bytes.is_some()));
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&(name_len as u32).to_le_bytes());
        if let Some(bytes) = name_bytes {
            data.extend_from_slice(bytes);
        }
        data.extend_from_slice(&pixels);
        FlatBufferTransferable::from_vec(WebMessageTag::LayerRaster, data, 0)
    }

    fn build_from_dem(
        coords: WorldTileCoords,
        source: RasterSourceId,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        _fallback: Arc<RgbaImage>,
    ) -> Self {
        let name_bytes = source.name().map(str::as_bytes);
        let name_len = name_bytes.map_or(0, <[u8]>::len);
        let pixels = tile.pixels();
        let mut data = Vec::with_capacity(RASTER_DEM_HEADER_BYTES + name_len + pixels.len());
        data.extend_from_slice(&coords.x.to_le_bytes());
        data.extend_from_slice(&coords.y.to_le_bytes());
        data.push(u8::from(coords.z));
        data.push(u8::from(name_bytes.is_some()));
        data.extend_from_slice(&RASTER_DEM_MAGIC.to_le_bytes());
        data.extend_from_slice(&tile.dim().to_le_bytes());
        data.extend_from_slice(&tile.stride().to_le_bytes());
        data.extend_from_slice(&(name_len as u32).to_le_bytes());
        for lane in tile.unpack() {
            data.extend_from_slice(&lane.to_le_bytes());
        }
        data.extend_from_slice(&tile.min().to_le_bytes());
        data.extend_from_slice(&tile.max().to_le_bytes());
        data.extend_from_slice(&circumference_meters.to_le_bytes());
        if let Some(bytes) = name_bytes {
            data.extend_from_slice(bytes);
        }
        data.extend_from_slice(pixels);
        FlatBufferTransferable::from_vec(WebMessageTag::LayerRaster, data, 0)
    }

    fn coords(&self) -> WorldTileCoords {
        self.data()
            .get(..RASTER_HEADER_BYTES)
            .and_then(|bytes| read_coords_at(bytes, 0))
            .unwrap_or_default()
    }

    fn to_layer(self) -> AvailableRasterLayerData {
        if is_raw_dem_frame(self.data()) {
            match decode_dem_frame(self) {
                Ok(shared) => AvailableRasterLayerData {
                    coords: shared.coords,
                    source: shared.source,
                    image: shared.tile.to_rgba_image(),
                },
                Err(layer) => layer,
            }
        } else {
            decode_rgba_layer(self)
        }
    }

    fn into_dem_or_layer(self) -> Result<SharedRasterDem, AvailableRasterLayerData> {
        if is_raw_dem_frame(self.data()) {
            decode_dem_frame(self)
        } else {
            Err(decode_rgba_layer(self))
        }
    }
}

impl LayerRasterMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerRasterMissing
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId) -> Self {
        encode_named_coords(WebMessageTag::LayerRasterMissing, coords, source.name())
    }

    fn coords(&self) -> WorldTileCoords {
        decode_named_coords(self.data()).0
    }

    fn to_layer(self) -> MissingRasterLayerData {
        let (coords, name) = decode_named_coords(self.data());
        MissingRasterLayerData {
            coords,
            source: RasterSourceId::new(name.map(str::to_owned)),
        }
    }
}

#[cfg(test)]
mod tests;

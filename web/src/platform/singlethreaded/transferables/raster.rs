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
    let has_name = bytes.get(9).is_some_and(|&b| b != 0);
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
    let has_name = bytes.get(9).is_some_and(|&b| b != 0);
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

#[cfg(any(test, feature = "renderer"))]
fn copy_header_bytes(
    buffer: &js_sys::ArrayBuffer,
    base_offset: u32,
    total_len: u32,
    need: usize,
) -> Option<Vec<u8>> {
    if (total_len as usize) < need {
        return None;
    }
    let mut hdr = vec![0_u8; need];
    js_sys::Uint8Array::new_with_byte_offset_and_length(buffer, base_offset, need as u32)
        .copy_to(&mut hdr);
    Some(hdr)
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn decode_bytes_layer_raster(bytes: &[u8]) -> Option<FlatBufferTransferable> {
    if is_raw_dem_frame(bytes) {
        let res = match parse_raw_raster_dem_header(bytes) {
            Ok(hdr) => {
                let pixels = bytes.get(hdr.px_offset..)?.to_vec();
                match DemTile::from_raw_parts(
                    hdr.dim, hdr.stride, pixels, hdr.unpack, hdr.min, hdr.max,
                ) {
                    Ok(dem) => Err(Box::new(SharedRasterDem {
                        coords: hdr.coords,
                        source: RasterSourceId::new(hdr.source_name),
                        tile: Arc::new(dem),
                        circumference_meters: hdr.circumference_meters,
                        fallback: Arc::new(RgbaImage::new(0, 0)),
                    })),
                    Err(_) => Ok(AvailableRasterLayerData {
                        coords: hdr.coords,
                        source: RasterSourceId::new(hdr.source_name),
                        image: RgbaImage::new(0, 0),
                    }),
                }
            }
            Err(layer) => Ok(layer),
        };
        return Some(FlatBufferTransferable::from_unpacked_raster(res));
    }
    if bytes.len() < RASTER_HEADER_BYTES {
        return None;
    }
    let coords = read_coords_at(bytes, 0)?;
    let has_name = bytes.get(9).is_some_and(|&b| b != 0);
    let (width, height, name_len) = (
        read_u32_le(bytes, 12)?,
        read_u32_le(bytes, 16)?,
        read_u32_le(bytes, 20)? as usize,
    );
    let px_offset = RASTER_HEADER_BYTES.checked_add(name_len)?;
    let source_name = has_name
        .then(|| {
            bytes
                .get(RASTER_HEADER_BYTES..px_offset)
                .and_then(|s| std::str::from_utf8(s).ok())
                .map(str::to_owned)
        })
        .flatten();
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))?;
    let px_slice = bytes.get(px_offset..)?;
    let image = if px_slice.len() == expected {
        RgbaImage::from_vec(width, height, px_slice.to_vec())
            .unwrap_or_else(|| RgbaImage::new(0, 0))
    } else {
        RgbaImage::new(0, 0)
    };
    Some(FlatBufferTransferable::from_unpacked_raster(Ok(
        AvailableRasterLayerData {
            coords,
            source: RasterSourceId::new(source_name),
            image,
        },
    )))
}

#[cfg(any(test, feature = "renderer"))]
fn decode_js_dem_or_rgba(
    is_dem: bool,
    hdr_bytes: &[u8],
    px_offset: usize,
    pixels: Vec<u8>,
) -> Option<Result<AvailableRasterLayerData, Box<SharedRasterDem>>> {
    if is_dem {
        return Some(match parse_raw_raster_dem_header(hdr_bytes) {
            Ok(hdr) => {
                match DemTile::from_raw_parts(
                    hdr.dim, hdr.stride, pixels, hdr.unpack, hdr.min, hdr.max,
                ) {
                    Ok(dem) => Err(Box::new(SharedRasterDem {
                        coords: hdr.coords,
                        source: RasterSourceId::new(hdr.source_name),
                        tile: Arc::new(dem),
                        circumference_meters: hdr.circumference_meters,
                        fallback: Arc::new(RgbaImage::new(0, 0)),
                    })),
                    Err(_) => Ok(AvailableRasterLayerData {
                        coords: hdr.coords,
                        source: RasterSourceId::new(hdr.source_name),
                        image: RgbaImage::new(0, 0),
                    }),
                }
            }
            Err(layer) => Ok(layer),
        });
    }
    let coords = read_coords_at(hdr_bytes, 0)?;
    let has_name = hdr_bytes.get(9).is_some_and(|&b| b != 0);
    let (width, height) = (read_u32_le(hdr_bytes, 12)?, read_u32_le(hdr_bytes, 16)?);
    let source_name = has_name
        .then(|| {
            hdr_bytes
                .get(RASTER_HEADER_BYTES..px_offset)
                .and_then(|s| std::str::from_utf8(s).ok())
                .map(str::to_owned)
        })
        .flatten();
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    let image = if expected == Some(pixels.len()) {
        RgbaImage::from_vec(width, height, pixels).unwrap_or_else(|| RgbaImage::new(0, 0))
    } else {
        RgbaImage::new(0, 0)
    };
    Some(Ok(AvailableRasterLayerData {
        coords,
        source: RasterSourceId::new(source_name),
        image,
    }))
}

#[cfg(any(test, feature = "renderer"))]
pub(super) fn decode_js_layer_raster(
    buffer: &js_sys::ArrayBuffer,
    base_offset: u32,
    byte_len: u32,
) -> Option<FlatBufferTransferable> {
    let end = base_offset.checked_add(byte_len)?;
    if end > buffer.byte_length() || (byte_len as usize) < RASTER_HEADER_BYTES {
        return None;
    }
    let mut peek = [0_u8; 256];
    let peek_len = (byte_len as usize).min(peek.len());
    js_sys::Uint8Array::new_with_byte_offset_and_length(buffer, base_offset, peek_len as u32)
        .copy_to(&mut peek[..peek_len]);
    let peek_slice = &peek[..peek_len];
    let is_dem = is_raw_dem_frame(peek_slice);
    let min_hdr = if is_dem {
        RASTER_DEM_HEADER_BYTES
    } else {
        RASTER_HEADER_BYTES
    };
    let name_len = read_u32_le(peek_slice, 20)? as usize;
    let px_offset = min_hdr.checked_add(name_len)?;
    if (byte_len as usize) < px_offset {
        return None;
    }
    let overflow_hdr;
    let hdr_bytes = if px_offset <= peek_len {
        &peek_slice[..px_offset]
    } else {
        overflow_hdr = copy_header_bytes(buffer, base_offset, byte_len, px_offset)?;
        overflow_hdr.as_slice()
    };
    let px_len = (byte_len as usize) - px_offset;
    let mut pixels = vec![0_u8; px_len];
    if px_len > 0 {
        js_sys::Uint8Array::new_with_byte_offset_and_length(
            buffer,
            base_offset + px_offset as u32,
            px_len as u32,
        )
        .copy_to(&mut pixels);
    }
    let res = decode_js_dem_or_rgba(is_dem, hdr_bytes, px_offset, pixels)?;
    Some(FlatBufferTransferable::from_unpacked_raster(res))
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
        if let Some(unpacked) = &self.unpacked_raster {
            return match unpacked {
                Ok(layer) => layer.coords,
                Err(dem) => dem.coords,
            };
        }
        self.data()
            .get(..RASTER_HEADER_BYTES)
            .and_then(|bytes| read_coords_at(bytes, 0))
            .unwrap_or_default()
    }

    fn to_layer(mut self) -> AvailableRasterLayerData {
        if let Some(unpacked) = self.unpacked_raster.take() {
            return match unpacked {
                Ok(layer) => layer,
                Err(shared) => AvailableRasterLayerData {
                    coords: shared.coords,
                    source: shared.source,
                    image: shared.tile.to_rgba_image(),
                },
            };
        }
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

    fn into_dem_or_layer(mut self) -> Result<SharedRasterDem, AvailableRasterLayerData> {
        if let Some(unpacked) = self.unpacked_raster.take() {
            return match unpacked {
                Ok(layer) => Err(layer),
                Err(shared) => Ok(*shared),
            };
        }
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

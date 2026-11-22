//! Elevation tiles crossing the worker boundary using zero-copy binary framing.

use std::sync::Arc;

use maplibre::terrain::DemTile;

use super::*;

const LAYER_DEM_IMAGE_MAGIC: u32 = 0x4D4C_4449;
const LAYER_DEM_RAW_MAGIC: u32 = 0x4D4C_444D;
const DEM_IMAGE_HEADER_BYTES: usize = 24;
const DEM_RAW_HEADER_BYTES: usize = 80;

struct RawDemHeader {
    dim: u32,
    stride: u32,
    unpack: [f64; 4],
    min: f64,
    max: f64,
    circumference: f64,
}

fn parse_raw_dem_header(bytes: &[u8]) -> Option<RawDemHeader> {
    if bytes.len() < DEM_RAW_HEADER_BYTES || read_u32_le(bytes, 0) != Some(LAYER_DEM_RAW_MAGIC) {
        return None;
    }
    read_coords_at(bytes, 4)?;
    let dim = read_u32_le(bytes, 16)?;
    let stride = read_u32_le(bytes, 20)?;
    let unpack = [
        read_f64_le(bytes, 24)?,
        read_f64_le(bytes, 32)?,
        read_f64_le(bytes, 40)?,
        read_f64_le(bytes, 48)?,
    ];
    let min = read_f64_le(bytes, 56)?;
    let max = read_f64_le(bytes, 64)?;
    let circumference = read_f64_le(bytes, 72)?;
    Some(RawDemHeader {
        dim,
        stride,
        unpack,
        min,
        max,
        circumference,
    })
}

impl LayerDem for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerDem
    }

    fn build_from(coords: WorldTileCoords, image: RgbaImage) -> Self {
        let width = image.width();
        let height = image.height();
        let pixels = image.into_vec();
        let mut data = Vec::with_capacity(DEM_IMAGE_HEADER_BYTES + pixels.len());
        data.extend_from_slice(&LAYER_DEM_IMAGE_MAGIC.to_le_bytes());
        data.extend_from_slice(&coords.x.to_le_bytes());
        data.extend_from_slice(&coords.y.to_le_bytes());
        data.push(u8::from(coords.z));
        data.extend_from_slice(&[0; 3]);
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&pixels);
        FlatBufferTransferable::from_vec(WebMessageTag::LayerDem, data, 0)
    }

    fn build_from_cached_dem(
        coords: WorldTileCoords,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        _fallback_image: Arc<RgbaImage>,
    ) -> Self {
        let pixels = tile.pixels();
        let mut data = Vec::with_capacity(DEM_RAW_HEADER_BYTES + pixels.len());
        data.extend_from_slice(&LAYER_DEM_RAW_MAGIC.to_le_bytes());
        data.extend_from_slice(&coords.x.to_le_bytes());
        data.extend_from_slice(&coords.y.to_le_bytes());
        data.push(u8::from(coords.z));
        data.extend_from_slice(&[0; 3]);
        data.extend_from_slice(&tile.dim().to_le_bytes());
        data.extend_from_slice(&tile.stride().to_le_bytes());
        for lane in tile.unpack() {
            data.extend_from_slice(&lane.to_le_bytes());
        }
        data.extend_from_slice(&tile.min().to_le_bytes());
        data.extend_from_slice(&tile.max().to_le_bytes());
        data.extend_from_slice(&circumference_meters.to_le_bytes());
        data.extend_from_slice(pixels);
        FlatBufferTransferable::from_vec(WebMessageTag::LayerDem, data, 0)
    }

    fn coords(&self) -> WorldTileCoords {
        let bytes = self.data();
        match read_u32_le(bytes, 0) {
            Some(LAYER_DEM_IMAGE_MAGIC | LAYER_DEM_RAW_MAGIC) => {
                read_coords_at(bytes, 4).unwrap_or_default()
            }
            _ => WorldTileCoords::default(),
        }
    }

    fn into_image(self) -> RgbaImage {
        let bytes = self.data();
        match read_u32_le(bytes, 0) {
            Some(LAYER_DEM_IMAGE_MAGIC) => {
                let (Some(width), Some(height)) = (read_u32_le(bytes, 16), read_u32_le(bytes, 20))
                else {
                    return RgbaImage::new(0, 0);
                };
                let expected = (width as usize)
                    .checked_mul(height as usize)
                    .and_then(|n| n.checked_mul(4));
                if expected != bytes.get(DEM_IMAGE_HEADER_BYTES..).map(<[u8]>::len) {
                    return RgbaImage::new(0, 0);
                }
                let pixels = self.into_tail_vec(DEM_IMAGE_HEADER_BYTES);
                RgbaImage::from_vec(width, height, pixels).unwrap_or_else(|| RgbaImage::new(0, 0))
            }
            Some(LAYER_DEM_RAW_MAGIC) => self
                .into_shared_dem()
                .map(|(dem, _, _)| dem.to_rgba_image())
                .unwrap_or_else(|image| image),
            _ => RgbaImage::new(0, 0),
        }
    }

    fn into_shared_dem(self) -> Result<(Arc<DemTile>, f64, Arc<RgbaImage>), RgbaImage> {
        let Some(hdr) = parse_raw_dem_header(self.data()) else {
            return Err(self.into_image());
        };
        let pixels = self.into_tail_vec(DEM_RAW_HEADER_BYTES);
        match DemTile::from_raw_parts(hdr.dim, hdr.stride, pixels, hdr.unpack, hdr.min, hdr.max) {
            Ok(dem) => Ok((
                Arc::new(dem),
                hdr.circumference,
                Arc::new(RgbaImage::new(0, 0)),
            )),
            Err(_) => Err(RgbaImage::new(0, 0)),
        }
    }
}

impl LayerDemMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerDemMissing
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        encode_named_coords(WebMessageTag::LayerDemMissing, coords, None)
    }

    fn coords(&self) -> WorldTileCoords {
        decode_named_coords(self.data()).0
    }
}

impl DemTransferables for FlatTransferables {
    type LayerDem = FlatBufferTransferable;
    type LayerDemMissing = FlatBufferTransferable;
}

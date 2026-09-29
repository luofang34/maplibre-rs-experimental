//! Raster image decoding and worker result delivery.

#![deny(missing_docs)]

use std::marker::PhantomData;

use image::RgbaImage;
use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    io::apc::{Context, SendError},
    raster::{
        transferables::{LayerRaster, RasterTransferables},
        RasterSourceId,
    },
};

/// Failure decoding image bytes or returning pixels to the caller.
#[derive(Error, Debug)]
pub enum ProcessRasterError {
    /// Source bytes could not be decoded as an image.
    #[error("decoding raster tile failed")]
    Decoding {
        /// Underlying image decoder error.
        #[source]
        source: image::ImageError,
    },
    /// The caller could not receive the decoded image.
    #[error("sending raster result failed")]
    Send(#[source] SendError),
}

/// Grid location associated with one source image.
pub struct RasterTileRequest {
    /// Tile whose image is being decoded.
    pub coords: WorldTileCoords,
    /// Source whose image is being decoded.
    pub source: RasterSourceId,
}

/// Decodes source bytes into RGBA8 pixels and sends them through the reply context.
/// Decoder and transport failures retain their original causes.
pub fn process_raster_tile<T: RasterTransferables, C: Context>(
    data: &[u8],
    tile_request: RasterTileRequest,
    context: &mut ProcessRasterContext<T, C>,
) -> Result<(), ProcessRasterError> {
    let coords = &tile_request.coords;
    let img =
        image::load_from_memory(data).map_err(|source| ProcessRasterError::Decoding { source })?;
    let rgba = img.to_rgba8();

    context.layer_raster_finished(coords, tile_request.source, rgba)?;

    Ok(())
}
/// Reply endpoint for decoded raster layers.
pub struct ProcessRasterContext<T: RasterTransferables, C: Context> {
    context: C,
    phantom_t: PhantomData<T>,
}

impl<T: RasterTransferables, C: Context> ProcessRasterContext<T, C> {
    /// Uses the supplied endpoint for each decoded layer.
    pub fn new(context: C) -> Self {
        Self {
            context,
            phantom_t: Default::default(),
        }
    }
}

impl<T: RasterTransferables, C: Context> ProcessRasterContext<T, C> {
    fn layer_raster_finished(
        &mut self,
        coords: &WorldTileCoords,
        source: RasterSourceId,
        image_data: RgbaImage,
    ) -> Result<(), ProcessRasterError> {
        self.context
            .send_back(T::LayerRaster::build_from(*coords, source, image_data))
            .map_err(ProcessRasterError::Send)
    }
}

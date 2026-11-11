//! Worker message contracts for decoded raster pixels and unavailable tiles.

#![deny(missing_docs)]

use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    raster::{AvailableRasterLayerData, MissingRasterLayerData, RasterSourceId},
    terrain::DemTile,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
/// Tags used by the owned Rust raster message implementations.
pub enum RasterMessageTag {
    /// A decoded tile image is available for upload.
    LayerRaster,
    /// A tile request completed without an image.
    LayerRasterMissing,
}

impl MessageTag for RasterMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// Shared worker-decoded DEM payload transported without copying texels in-process.
pub struct SharedRasterDem {
    /// Tile-grid coordinates covered by the DEM tile.
    pub coords: WorldTileCoords,
    /// Style `raster-dem` source owning these elevation samples.
    pub source: RasterSourceId,
    /// Pre-decoded 2-texel bordered elevation tile.
    pub tile: Arc<DemTile>,
    /// Body circumference in metres used when despiking `tile`.
    pub circumference_meters: f64,
    /// Original RGBA8 image retained as a fallback if encoding parameters changed.
    pub fallback: Arc<RgbaImage>,
}

/// Decoded RGBA8 pixels ready to become a renderer raster layer.
pub trait LayerRaster: IntoMessage + Debug + Send {
    /// Identifies this backend's decoded-image payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Takes ownership of a decoded image and its source identity at the supplied tile coordinates.
    fn build_from(coords: WorldTileCoords, source: RasterSourceId, image: RgbaImage) -> Self;

    /// Wraps a worker-decoded [`DemTile`] for `raster-dem` sources when supported by the transport,
    /// falling back to `fallback` for serialized cross-worker transports.
    fn build_from_dem(
        coords: WorldTileCoords,
        source: RasterSourceId,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback: Arc<RgbaImage>,
    ) -> Self
    where
        Self: Sized,
    {
        let _ = (tile, circumference_meters);
        Self::build_from(coords, source, (*fallback).clone())
    }

    /// Tile-grid coordinates covered by the image.
    fn coords(&self) -> WorldTileCoords;

    /// Consumes the message into pixel data awaiting GPU upload.
    fn to_layer(self) -> AvailableRasterLayerData;

    /// Extracts a shared pre-decoded [`DemTile`] when carried in-process, or falls back to [`Self::to_layer`].
    fn into_dem_or_layer(self) -> Result<SharedRasterDem, AvailableRasterLayerData>
    where
        Self: Sized,
    {
        Err(self.to_layer())
    }
}

/// A raster source response without pixels; the request outcome determines retry eligibility.
pub trait LayerRasterMissing: IntoMessage + Debug + Send {
    /// Identifies this backend's missing-image payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Records an unavailable raster tile at the supplied grid coordinates.
    fn build_from(coords: WorldTileCoords, source: RasterSourceId) -> Self;

    /// Tile-grid coordinates of the unsuccessful request.
    fn coords(&self) -> WorldTileCoords;

    /// Consumes the message into the renderer's missing-layer record.
    fn to_layer(self) -> MissingRasterLayerData;
}

enum DefaultRasterPayload {
    Image(RgbaImage),
    Dem {
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback: Arc<RgbaImage>,
    },
}

/// Owned image or shared DEM message retaining the identity of the requested source.
pub struct DefaultLayerRaster {
    /// Tile-grid coordinates covered by the image.
    pub coords: WorldTileCoords,
    /// Source owning these pixels.
    pub source: RasterSourceId,
    payload: DefaultRasterPayload,
}

impl Debug for DefaultLayerRaster {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultRasterLayer({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerRaster {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerRaster for DefaultLayerRaster {
    fn message_tag() -> &'static dyn MessageTag {
        &RasterMessageTag::LayerRaster
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId, image: RgbaImage) -> Self {
        Self {
            coords,
            source,
            payload: DefaultRasterPayload::Image(image),
        }
    }

    fn build_from_dem(
        coords: WorldTileCoords,
        source: RasterSourceId,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback: Arc<RgbaImage>,
    ) -> Self {
        Self {
            coords,
            source,
            payload: DefaultRasterPayload::Dem {
                tile,
                circumference_meters,
                fallback,
            },
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn to_layer(self) -> AvailableRasterLayerData {
        let image = match self.payload {
            DefaultRasterPayload::Image(image) => image,
            DefaultRasterPayload::Dem { fallback, .. } => {
                Arc::try_unwrap(fallback).unwrap_or_else(|shared| (*shared).clone())
            }
        };
        AvailableRasterLayerData {
            coords: self.coords,
            source: self.source,
            image,
        }
    }

    fn into_dem_or_layer(self) -> Result<SharedRasterDem, AvailableRasterLayerData> {
        match self.payload {
            DefaultRasterPayload::Dem {
                tile,
                circumference_meters,
                fallback,
            } => Ok(SharedRasterDem {
                coords: self.coords,
                source: self.source,
                tile,
                circumference_meters,
                fallback,
            }),
            DefaultRasterPayload::Image(image) => Err(AvailableRasterLayerData {
                coords: self.coords,
                source: self.source,
                image,
            }),
        }
    }
}

/// Owned unavailable-tile message retaining the requested source identity.
pub struct DefaultLayerRasterMissing {
    /// Tile-grid coordinates of the unsuccessful request.
    pub coords: WorldTileCoords,
    /// Source whose request yielded no image.
    pub source: RasterSourceId,
}

impl Debug for DefaultLayerRasterMissing {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultRasterLayerMissing({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerRasterMissing {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerRasterMissing for DefaultLayerRasterMissing {
    fn message_tag() -> &'static dyn MessageTag {
        &RasterMessageTag::LayerRasterMissing
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId) -> Self {
        Self { coords, source }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn to_layer(self) -> MissingRasterLayerData {
        MissingRasterLayerData {
            coords: self.coords,
            source: self.source,
        }
    }
}

/// Selects matching payload representations for raster workers and their receivers.
pub trait RasterTransferables: Copy + Clone + 'static {
    /// Decoded image ready for upload.
    type LayerRaster: LayerRaster;
    /// Tile request completed without an image.
    type LayerRasterMissing: LayerRasterMissing;
}

#[derive(Copy, Clone)]
/// Uses owned Rust pixel buffers without serializing them for worker transport.
pub struct DefaultRasterTransferables;

impl RasterTransferables for DefaultRasterTransferables {
    type LayerRaster = DefaultLayerRaster;
    type LayerRasterMissing = DefaultLayerRasterMissing;
}

#[cfg(test)]
mod tests;

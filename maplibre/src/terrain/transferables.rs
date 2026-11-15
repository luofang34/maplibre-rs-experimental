//! Messages carrying DEM tiles from workers to the map thread.

use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    terrain::dem::DemTile,
};

/// Tags of the messages the DEM pipeline sends back from workers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DemMessageTag {
    /// A decoded DEM image.
    LayerDem,
    /// A DEM tile that could not be fetched or decoded.
    LayerDemMissing,
}

impl MessageTag for DemMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// A decoded DEM tile image on its way to the map thread.
pub trait LayerDem: IntoMessage + Debug + Send {
    /// Tag identifying this message kind.
    fn message_tag() -> &'static dyn MessageTag;

    /// Wraps a decoded image.
    fn build_from(coords: WorldTileCoords, image: RgbaImage) -> Self;

    /// Wraps a pre-decoded shared [`DemTile`] when the transport runs in-process, falling back
    /// to `fallback_image` (or reconstructing the interior image from `tile` when `fallback_image`
    /// is an empty placeholder) for cross-worker transports.
    fn build_from_shared_dem(
        coords: WorldTileCoords,
        tile: Arc<DemTile>,
        fallback_image: &RgbaImage,
    ) -> Self
    where
        Self: Sized,
    {
        Self::build_from(coords, tile.resolve_fallback_image(fallback_image))
    }

    /// Coordinates of the DEM tile.
    fn coords(&self) -> WorldTileCoords;

    /// Unwraps the decoded image.
    fn into_image(self) -> RgbaImage;

    /// Extracts a shared pre-decoded [`DemTile`] if this transport carries one, or falls back to
    /// [`Self::into_image`].
    fn into_shared_dem(self) -> Result<(Arc<DemTile>, f64, Arc<RgbaImage>), RgbaImage>
    where
        Self: Sized,
    {
        Err(self.into_image())
    }

    /// Wraps a pre-decoded shared [`DemTile`] together with the body circumference (in metres)
    /// used when despiking it.
    fn build_from_cached_dem(
        coords: WorldTileCoords,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback_image: Arc<RgbaImage>,
    ) -> Self
    where
        Self: Sized,
    {
        let _ = circumference_meters;
        Self::build_from_shared_dem(coords, tile, &fallback_image)
    }
}

/// A DEM tile that will never arrive.
pub trait LayerDemMissing: IntoMessage + Debug + Send {
    /// Tag identifying this message kind.
    fn message_tag() -> &'static dyn MessageTag;

    /// Marks the tile as missing.
    fn build_from(coords: WorldTileCoords) -> Self;

    /// Coordinates of the DEM tile.
    fn coords(&self) -> WorldTileCoords;
}

enum DefaultDemPayload {
    Image(RgbaImage),
    Shared {
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback: Arc<RgbaImage>,
    },
}

/// In-process DEM message.
pub struct DefaultLayerDem {
    coords: WorldTileCoords,
    payload: DefaultDemPayload,
}

impl Debug for DefaultLayerDem {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultLayerDem({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerDem {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerDem for DefaultLayerDem {
    fn message_tag() -> &'static dyn MessageTag {
        &DemMessageTag::LayerDem
    }

    fn build_from(coords: WorldTileCoords, image: RgbaImage) -> Self {
        Self {
            coords,
            payload: DefaultDemPayload::Image(image),
        }
    }

    fn build_from_cached_dem(
        coords: WorldTileCoords,
        tile: Arc<DemTile>,
        circumference_meters: f64,
        fallback_image: Arc<RgbaImage>,
    ) -> Self {
        Self {
            coords,
            payload: DefaultDemPayload::Shared {
                tile,
                circumference_meters,
                fallback: fallback_image,
            },
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn into_image(self) -> RgbaImage {
        match self.payload {
            DefaultDemPayload::Image(image) => image,
            DefaultDemPayload::Shared { tile, fallback, .. } => {
                if fallback.width() == 0 || fallback.height() == 0 {
                    tile.to_rgba_image()
                } else {
                    Arc::try_unwrap(fallback).unwrap_or_else(|shared| (*shared).clone())
                }
            }
        }
    }

    fn into_shared_dem(self) -> Result<(Arc<DemTile>, f64, Arc<RgbaImage>), RgbaImage> {
        match self.payload {
            DefaultDemPayload::Shared {
                tile,
                circumference_meters,
                fallback,
            } => Ok((tile, circumference_meters, fallback)),
            DefaultDemPayload::Image(image) => Err(image),
        }
    }
}

/// In-process message for a missing DEM tile.
pub struct DefaultLayerDemMissing {
    coords: WorldTileCoords,
}

impl Debug for DefaultLayerDemMissing {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultLayerDemMissing({})", self.coords)
    }
}

impl IntoMessage for DefaultLayerDemMissing {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self))
    }
}

impl LayerDemMissing for DefaultLayerDemMissing {
    fn message_tag() -> &'static dyn MessageTag {
        &DemMessageTag::LayerDemMissing
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        Self { coords }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }
}

/// Message types a platform uses to transport DEM tiles.
pub trait DemTransferables: Copy + Clone + 'static {
    /// Decoded DEM image message.
    type LayerDem: LayerDem;
    /// Missing DEM tile message.
    type LayerDemMissing: LayerDemMissing;
}

/// In-process DEM transport.
#[derive(Copy, Clone)]
pub struct DefaultDemTransferables;

impl DemTransferables for DefaultDemTransferables {
    type LayerDem = DefaultLayerDem;
    type LayerDemMissing = DefaultLayerDemMissing;
}

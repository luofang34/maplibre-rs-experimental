//! Messages carrying DEM tiles from workers to the map thread.

use std::fmt::{Debug, Formatter};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
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

    /// Coordinates of the DEM tile.
    fn coords(&self) -> WorldTileCoords;

    /// Unwraps the decoded image.
    fn into_image(self) -> RgbaImage;
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

/// In-process DEM message.
pub struct DefaultLayerDem {
    coords: WorldTileCoords,
    image: RgbaImage,
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
        Self { coords, image }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn into_image(self) -> RgbaImage {
        self.image
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

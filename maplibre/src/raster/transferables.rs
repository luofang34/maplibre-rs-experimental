//! Worker message contracts for decoded raster pixels and unavailable tiles.

#![deny(missing_docs)]

use std::fmt::{Debug, Formatter};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    raster::{AvailableRasterLayerData, MissingRasterLayerData},
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

/// Decoded RGBA8 pixels ready to become a renderer raster layer.
pub trait LayerRaster: IntoMessage + Debug + Send {
    /// Identifies this backend's decoded-image payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Takes ownership of a decoded image and its request label at the supplied tile coordinates.
    fn build_from(coords: WorldTileCoords, layer_name: String, image: RgbaImage) -> Self;

    /// Tile-grid coordinates covered by the image.
    fn coords(&self) -> WorldTileCoords;

    /// Consumes the message into pixel data awaiting GPU upload.
    fn to_layer(self) -> AvailableRasterLayerData;
}

/// A completed raster request with no image, so the tile need not be requested repeatedly.
pub trait LayerRasterMissing: IntoMessage + Debug + Send {
    /// Identifies this backend's missing-image payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Records an unavailable raster tile at the supplied grid coordinates.
    fn build_from(coords: WorldTileCoords) -> Self;

    /// Tile-grid coordinates of the unsuccessful request.
    fn coords(&self) -> WorldTileCoords;

    /// Consumes the message into the renderer's missing-layer record.
    fn to_layer(self) -> MissingRasterLayerData;
}

/// Owned image message; conversion uses the renderer's `raster` source-layer key.
pub struct DefaultLayerRaster {
    /// Tile-grid coordinates covered by the image.
    pub coords: WorldTileCoords,
    /// Request label retained in this payload; conversion to a layer uses `raster` instead.
    pub layer_name: String,
    /// Decoded RGBA8 pixels, before GPU upload.
    pub image: RgbaImage,
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

    fn build_from(coords: WorldTileCoords, layer_name: String, image: RgbaImage) -> Self {
        Self {
            coords,
            layer_name,
            image,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn to_layer(self) -> AvailableRasterLayerData {
        AvailableRasterLayerData {
            coords: self.coords,
            source_layer: "raster".to_string(),
            image: self.image,
        }
    }
}

/// Owned unavailable-tile message using the renderer's `raster` source-layer key.
pub struct DefaultLayerRasterMissing {
    /// Tile-grid coordinates of the unsuccessful request.
    pub coords: WorldTileCoords,
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

    fn build_from(coords: WorldTileCoords) -> Self {
        Self { coords }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn to_layer(self) -> MissingRasterLayerData {
        MissingRasterLayerData {
            coords: self.coords,
            source_layer: "raster".to_string(),
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

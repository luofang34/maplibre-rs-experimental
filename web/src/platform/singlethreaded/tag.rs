//! Wire message tags for single-threaded web worker APC payloads.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use maplibre::io::apc::MessageTag;
use thiserror::Error;

/// Error which happens during serialization or deserialization of the tag
#[derive(Error, Debug)]
#[error("failed to deserialize message tag")]
pub struct MessageTagDeserializeError;

impl MessageTag for WebMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum WebMessageTag {
    TileTessellated = 1,
    LayerMissing = 2,
    LayerTessellated = 3,
    LayerIndexed = 4,
    LayerRaster = 5,
    LayerRasterMissing = 6,
    SymbolLayerTessellated = 7,
    LayerDem = 8,
    LayerDemMissing = 9,
    TileRequestOutcome = 10,
    TrackedPayload = 11,
    ProvidedImages = 12,
    /// A worker stopped running the call for the attempt in the payload, either way.
    CallFinished = 13,
    /// Packed sequence of per-tile layer payloads and their trailing completion marker.
    BatchPayload = 14,
}

impl WebMessageTag {
    pub const fn is_intermediate_layer(self) -> bool {
        matches!(
            self,
            WebMessageTag::LayerMissing
                | WebMessageTag::LayerTessellated
                | WebMessageTag::LayerIndexed
                | WebMessageTag::SymbolLayerTessellated
        )
    }

    pub fn to_static(self) -> &'static WebMessageTag {
        match self {
            WebMessageTag::LayerRaster => &WebMessageTag::LayerRaster,
            WebMessageTag::LayerMissing => &WebMessageTag::LayerMissing,
            WebMessageTag::LayerIndexed => &WebMessageTag::LayerIndexed,
            WebMessageTag::TileTessellated => &WebMessageTag::TileTessellated,
            WebMessageTag::LayerTessellated => &WebMessageTag::LayerTessellated,
            WebMessageTag::LayerRasterMissing => &WebMessageTag::LayerRasterMissing,
            WebMessageTag::SymbolLayerTessellated => &WebMessageTag::SymbolLayerTessellated,
            WebMessageTag::LayerDem => &WebMessageTag::LayerDem,
            WebMessageTag::LayerDemMissing => &WebMessageTag::LayerDemMissing,
            WebMessageTag::TileRequestOutcome => &WebMessageTag::TileRequestOutcome,
            WebMessageTag::TrackedPayload => &WebMessageTag::TrackedPayload,
            WebMessageTag::ProvidedImages => &WebMessageTag::ProvidedImages,
            WebMessageTag::CallFinished => &WebMessageTag::CallFinished,
            WebMessageTag::BatchPayload => &WebMessageTag::BatchPayload,
        }
    }

    pub fn from_u32(tag: u32) -> Result<Self, MessageTagDeserializeError> {
        match tag {
            x if x == WebMessageTag::LayerMissing as u32 => Ok(WebMessageTag::LayerMissing),
            x if x == WebMessageTag::LayerTessellated as u32 => Ok(WebMessageTag::LayerTessellated),
            x if x == WebMessageTag::TileTessellated as u32 => Ok(WebMessageTag::TileTessellated),
            x if x == WebMessageTag::LayerIndexed as u32 => Ok(WebMessageTag::LayerIndexed),
            x if x == WebMessageTag::LayerRaster as u32 => Ok(WebMessageTag::LayerRaster),
            x if x == WebMessageTag::LayerRasterMissing as u32 => {
                Ok(WebMessageTag::LayerRasterMissing)
            }
            x if x == WebMessageTag::SymbolLayerTessellated as u32 => {
                Ok(WebMessageTag::SymbolLayerTessellated)
            }
            x if x == WebMessageTag::LayerDem as u32 => Ok(WebMessageTag::LayerDem),
            x if x == WebMessageTag::LayerDemMissing as u32 => Ok(WebMessageTag::LayerDemMissing),
            x if x == WebMessageTag::TileRequestOutcome as u32 => {
                Ok(WebMessageTag::TileRequestOutcome)
            }
            x if x == WebMessageTag::TrackedPayload as u32 => Ok(WebMessageTag::TrackedPayload),
            x if x == WebMessageTag::ProvidedImages as u32 => Ok(WebMessageTag::ProvidedImages),
            x if x == WebMessageTag::CallFinished as u32 => Ok(WebMessageTag::CallFinished),
            x if x == WebMessageTag::BatchPayload as u32 => Ok(WebMessageTag::BatchPayload),
            _ => Err(MessageTagDeserializeError),
        }
    }
}

impl From<WebMessageTag> for u32 {
    fn from(val: WebMessageTag) -> Self {
        val as u32
    }
}

//! Worker message contract for symbol tessellation payloads.

use std::fmt::{Debug, Formatter};

use geozero::mvt::tile::Layer;

use crate::{
    coords::WorldTileCoords,
    io::apc::{IntoMessage, Message, MessageTag},
    render::{
        resource::{IndexDataType, OverAlignedVertexBuffer},
        shaders::ShaderSymbolVertex,
    },
    sdf::{Feature, SymbolLayerData},
};

/// Tag identifying [`DefaultSymbolLayerTessellated`] messages across worker boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct SymbolMessageTag;

impl MessageTag for SymbolMessageTag {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// Text or icon geometry with placement metadata and an optional shared glyph/sprite atlas.
pub trait SymbolLayerTessellated: IntoMessage + Debug + Send {
    /// Identifies this backend's symbol payload for message dispatch.
    fn message_tag() -> &'static dyn MessageTag;

    /// Takes ownership of symbol geometry and feature ranges for one source/style layer pair.
    /// Atlas ownership is shared so its texels remain available when the renderer uploads them.
    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self
    where
        Self: Sized;

    /// Tile-grid coordinates whose local space contains the symbol anchors.
    fn coords(&self) -> WorldTileCoords;

    /// Whether there are no usable symbol draw indices, excluding alignment padding.
    fn is_empty(&self) -> bool;

    /// Consumes the message into the renderer's symbol layer data.
    fn to_bucket(self) -> SymbolLayerData;
}

/// Selects the symbol-tessellation message type used by an [`SdfPlugin`](crate::sdf::SdfPlugin).
pub trait SymbolTransferables: Copy + Clone + 'static {
    /// Text/icon geometry, atlas and placement metadata.
    type SymbolLayerTessellated: SymbolLayerTessellated;
}

/// Owned symbol geometry and placement metadata with shared atlas ownership.
pub struct DefaultSymbolLayerTessellated {
    /// Glyph and sprite texels shared with other messages using the same atlas.
    pub atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
    /// Tile-grid coordinates whose local space contains the symbol anchors.
    pub coords: WorldTileCoords,
    /// Padded symbol vertex/index storage and usable draw index count.
    pub buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
    /// Placement metadata and geometry ranges for the symbols in this buffer.
    pub features: Vec<Feature>,
    /// Decoded source layer; bucket conversion retains its name.
    pub layer_data: Layer,
    /// Style entry whose symbol layout produced the geometry.
    pub style_layer_id: String,
}

impl Debug for DefaultSymbolLayerTessellated {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DefaultSymbolLayerTessellated({})", self.coords)
    }
}

impl IntoMessage for DefaultSymbolLayerTessellated {
    fn into(self) -> Message {
        Message::new(Self::message_tag(), Box::new(self)).with_intermediate(true)
    }
}

impl SymbolLayerTessellated for DefaultSymbolLayerTessellated {
    fn message_tag() -> &'static dyn MessageTag {
        &SymbolMessageTag
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<crate::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        Self {
            atlas,
            coords,
            buffer,
            features,
            layer_data,
            style_layer_id,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        self.coords
    }

    fn is_empty(&self) -> bool {
        self.buffer.usable_indices == 0
    }

    fn to_bucket(self) -> SymbolLayerData {
        SymbolLayerData {
            atlas: self.atlas,
            coords: self.coords,
            source_layer: self.layer_data.name,
            style_layer_id: self.style_layer_id,
            buffer: self.buffer,
            features: self.features,
        }
    }
}

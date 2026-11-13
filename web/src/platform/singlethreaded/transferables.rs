use std::fmt::{Debug, Formatter};

use flatbuffers::FlatBufferBuilder;
use image::RgbaImage;
use js_sys::{ArrayBuffer, Uint8Array};
use maplibre::{
    benchmarking::tessellation::{IndexDataType, OverAlignedVertexBuffer},
    coords::WorldTileCoords,
    io::{
        apc::{IntoMessage, Message, MessageTag},
        geometry_index::TileIndex,
    },
    raster::{
        AvailableRasterLayerData, LayerRaster, LayerRasterMissing, MissingRasterLayerData,
        RasterSourceId, RasterTransferables,
    },
    render::shaders::ShaderSymbolVertex,
    sdf::{Feature, SymbolLayerData},
    terrain::transferables::{DemTransferables, LayerDem, LayerDemMissing},
    tile::Layer,
    vector::{
        LayerIndexed, LayerMissing, MissingVectorLayerBucket, SymbolLayerTessellated,
        TileTessellated, VectorTransferables,
    },
};

use crate::platform::singlethreaded::{
    apc::WebMessageTag,
    transferables::{
        basic_generated::*, layer_dem_generated::*, layer_dem_missing_generated::*,
        layer_indexed_generated::*, layer_missing_generated::*, layer_raster_generated::*,
        symbol_layer_tessellated_generated::*, tile_tessellated_generated::*,
    },
};

pub mod basic_generated {
    #![allow(unused, unused_imports, clippy::all)]

    use maplibre::coords::{WorldTileCoords, ZoomLevel};

    include!(concat!(env!("OUT_DIR"), "/basic_generated.rs"));

    impl Into<WorldTileCoords> for &FlatWorldTileCoords {
        fn into(self) -> WorldTileCoords {
            WorldTileCoords {
                x: self.x(),
                y: self.y(),
                z: ZoomLevel::new(self.z()),
            }
        }
    }
}
pub mod layer_indexed_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_indexed_generated.rs"));
}
pub mod layer_tessellated_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_tessellated_generated.rs"));
}
pub mod layer_missing_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_missing_generated.rs"));
}
pub mod tile_tessellated_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/tile_tessellated_generated.rs"));
}
pub mod layer_raster_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_raster_generated.rs"));
}

pub mod layer_dem_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_dem_generated.rs"));
}

pub mod layer_dem_missing_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/layer_dem_missing_generated.rs"));
}

pub mod symbol_layer_tessellated_generated {
    #![allow(unused, unused_imports, clippy::all)]
    include!(concat!(
        env!("OUT_DIR"),
        "/symbol_layer_tessellated_generated.rs"
    ));
}

pub struct FlatBufferTransferable {
    tag: WebMessageTag,
    data: Vec<u8>,
    start: usize,
}

impl FlatBufferTransferable {
    pub fn from_array_buffer(tag: WebMessageTag, buffer: ArrayBuffer) -> Self {
        let buffer = Uint8Array::new(&buffer);

        FlatBufferTransferable {
            tag,
            data: buffer.to_vec(),
            start: 0,
        }
    }

    pub fn from_vec(tag: WebMessageTag, data: Vec<u8>, start: usize) -> Self {
        FlatBufferTransferable { tag, data, start }
    }

    pub fn data(&self) -> &[u8] {
        &self.data[self.start..]
    }
}

impl TileTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::TileTessellated
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        tile_completion(coords, false, false)
    }
    fn build_partial(coords: WorldTileCoords) -> Self {
        tile_completion(coords, true, false)
    }
    fn build_failed(coords: WorldTileCoords, pending_symbols: bool) -> Self {
        tile_completion(coords, pending_symbols, true)
    }
    fn failed(&self) -> bool {
        root_as_flat_tile_tessellated(&self.data[self.start..])
            .map(|data| data.failed())
            .unwrap_or(true)
    }
    fn pending_symbols(&self) -> bool {
        root_as_flat_tile_tessellated(&self.data[self.start..])
            .map(|data| data.pending_symbols())
            .unwrap_or(false)
    }

    fn coords(&self) -> WorldTileCoords {
        let data = root_as_flat_tile_tessellated(&self.data[self.start..]).unwrap();
        data.coords().unwrap().into()
    }
}

impl LayerMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerMissing
    }

    fn build_from(coords: WorldTileCoords, layer_name: String) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(128);
        let layer_name = inner_builder.create_string(&layer_name);

        let mut builder = FlatLayerMissingBuilder::new(&mut inner_builder);
        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        builder.add_layer_name(layer_name);
        let root = builder.finish();

        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerMissing,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        let data = root_as_flat_layer_missing(&self.data[self.start..]).unwrap();
        data.coords().unwrap().into()
    }

    fn layer_name(&self) -> &str {
        let data = root_as_flat_layer_missing(&self.data[self.start..]).unwrap();
        data.layer_name().expect("property must be set")
    }

    fn to_bucket(self) -> MissingVectorLayerBucket {
        MissingVectorLayerBucket {
            source_layer: self.layer_name().to_owned(),
            coords: LayerMissing::coords(&self),
        }
    }
}

impl Debug for FlatBufferTransferable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "FlatBufferTransferable<{:?}>(??)", self.tag)
    }
}

impl IntoMessage for FlatBufferTransferable {
    fn into(self) -> Message {
        Message::new(self.tag.to_static(), Box::new(self))
    }
}

mod indexed;
mod tessellated;

mod raster;

mod symbol;

#[derive(Copy, Clone)]
pub struct FlatTransferables;

impl VectorTransferables for FlatTransferables {
    type TileTessellated = FlatBufferTransferable;
    type LayerMissing = FlatBufferTransferable;
    type LayerTessellated = FlatBufferTransferable;
    type SymbolLayerTessellated = FlatBufferTransferable;
    type LayerIndexed = FlatBufferTransferable;
}

impl RasterTransferables for FlatTransferables {
    type LayerRaster = FlatBufferTransferable;
    type LayerRasterMissing = FlatBufferTransferable;
}

mod terrain;

mod completion;
use self::completion::tile_completion;

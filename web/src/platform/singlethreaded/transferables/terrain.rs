//! Elevation tiles crossing the worker boundary.
use super::*;

impl LayerDem for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerDem
    }

    fn build_from(coords: WorldTileCoords, image: RgbaImage) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(1024);
        let width = image.width();
        let height = image.height();
        let image_data = inner_builder.create_vector(&image.into_vec());
        let mut builder = FlatLayerDemBuilder::new(&mut inner_builder);
        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        builder.add_image_data(image_data);
        builder.add_width(width);
        builder.add_height(height);
        let root = builder.finish();
        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerDem,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_dem(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }

    fn into_image(self) -> RgbaImage {
        let Ok(data) = root_as_flat_layer_dem(&self.data[self.start..]) else {
            return RgbaImage::new(0, 0);
        };
        data.image_data()
            .map(|bytes| bytes.iter().collect())
            .and_then(|image_data| RgbaImage::from_vec(data.width(), data.height(), image_data))
            .unwrap_or_else(|| RgbaImage::new(0, 0))
    }
}

impl LayerDemMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerDemMissing
    }

    fn build_from(coords: WorldTileCoords) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(1024);
        let mut builder = FlatLayerDemMissingBuilder::new(&mut inner_builder);
        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        let root = builder.finish();
        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerDemMissing,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_dem_missing(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }
}

impl DemTransferables for FlatTransferables {
    type LayerDem = FlatBufferTransferable;
    type LayerDemMissing = FlatBufferTransferable;
}

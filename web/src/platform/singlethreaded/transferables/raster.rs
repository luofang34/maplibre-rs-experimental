//! Source identities retained across raster worker buffer transfers.
use super::*;

impl LayerRaster for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerRaster
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId, image: RgbaImage) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(1024);

        let width = image.width();
        let height = image.height();

        let layer_name = source.name().map(|name| inner_builder.create_string(name));
        let image_data = inner_builder.create_vector(&image.into_vec());

        let mut builder = FlatLayerRasterBuilder::new(&mut inner_builder);

        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        if let Some(layer_name) = layer_name {
            builder.add_layer_name(layer_name);
        }
        builder.add_image_data(image_data);
        builder.add_width(width);
        builder.add_height(height);

        let root = builder.finish();
        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerRaster,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_raster(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }

    fn to_layer(self) -> AvailableRasterLayerData {
        let coords = LayerRaster::coords(&self);
        let Ok(data) = root_as_flat_layer_raster(&self.data[self.start..]) else {
            return AvailableRasterLayerData {
                coords,
                source: RasterSourceId::default(),
                image: RgbaImage::new(0, 0),
            };
        };
        let source = RasterSourceId::new(data.layer_name().map(str::to_owned));
        let image = data
            .image_data()
            .map(|bytes| bytes.iter().collect())
            .and_then(|image_data| RgbaImage::from_vec(data.width(), data.height(), image_data))
            .unwrap_or_else(|| RgbaImage::new(0, 0));
        AvailableRasterLayerData {
            coords,
            source,
            image,
        }
    }
}

impl LayerRasterMissing for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerRasterMissing
    }

    fn build_from(coords: WorldTileCoords, source: RasterSourceId) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(1024);
        let name = source.name().map(|name| inner_builder.create_string(name));
        let mut builder = FlatLayerMissingBuilder::new(&mut inner_builder);
        if let Some(name) = name {
            builder.add_layer_name(name);
        }

        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        let root = builder.finish();
        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerRasterMissing,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_missing(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }

    fn to_layer(self) -> MissingRasterLayerData {
        let coords = LayerRasterMissing::coords(&self);
        let source = root_as_flat_layer_missing(&self.data[self.start..])
            .ok()
            .map(|data| RasterSourceId::new(data.layer_name().map(str::to_owned)))
            .unwrap_or_default();
        MissingRasterLayerData { coords, source }
    }
}

#[cfg(test)]
mod tests;

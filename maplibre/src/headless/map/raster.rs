//! Merges supplied source images without replacing other sources at the same coordinates.

use std::collections::BTreeMap;

use image::RgbaImage;

use super::{HeadlessMap, HeadlessMapOperationError};
use crate::{
    raster::{
        resource::RasterResources, AvailableRasterLayerData, MissingRasterLayerData,
        RasterLayerData, RasterLayersDataComponent,
    },
    render::eventually::Eventually,
    terrain::dem_store::{
        decode_image_to_dem, dem_unpack_for_source, ingest_source_dem, is_terrain_source,
    },
};

impl HeadlessMap {
    pub(super) fn load_raster_layers(
        &mut self,
        raster_layers: Vec<AvailableRasterLayerData>,
    ) -> Result<(), HeadlessMapOperationError> {
        let mut rasters_by_tile = BTreeMap::new();
        for layer in raster_layers {
            rasters_by_tile
                .entry(layer.coords)
                .or_insert_with(Vec::new)
                .push(layer);
        }
        let style = &self.map_context.style;
        let circumference = self.map_context.view_state.body().circumference_meters();
        let world = &mut self.map_context.world;
        for (coords, layers) in rasters_by_tile {
            if world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_none()
            {
                world
                    .tiles
                    .spawn_mut(coords)
                    .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                    .insert(RasterLayersDataComponent::default());
            }
            for layer in layers {
                let source = layer.source.clone();
                let record = if let Some(unpack) = dem_unpack_for_source(style, &source) {
                    match decode_image_to_dem(coords, &layer.image, unpack, circumference) {
                        Some(dem) => {
                            ingest_source_dem(
                                &mut world.tiles,
                                &source,
                                coords,
                                dem,
                                is_terrain_source(style, &source),
                            );
                            RasterLayerData::Available(AvailableRasterLayerData {
                                coords,
                                source: source.clone(),
                                image: RgbaImage::new(0, 0),
                            })
                        }
                        None => RasterLayerData::Missing(MissingRasterLayerData {
                            coords,
                            source: source.clone(),
                        }),
                    }
                } else {
                    RasterLayerData::Available(layer)
                };
                let changed = world
                    .tiles
                    .query_mut::<&mut RasterLayersDataComponent>(coords)
                    .is_some_and(|component| component.record(record));
                if changed {
                    if let Some(Eventually::Initialized(raster)) =
                        world.resources.get_mut::<Eventually<RasterResources>>()
                    {
                        raster.tile_data_changed(&source, coords);
                    }
                }
            }
        }
        Ok(())
    }
}

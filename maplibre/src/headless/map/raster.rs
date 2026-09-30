//! Merges supplied source images without replacing other sources at the same coordinates.

use std::collections::BTreeMap;

use super::{HeadlessMap, HeadlessMapOperationError};
use crate::{
    raster::{
        resource::RasterResources, AvailableRasterLayerData, RasterLayerData,
        RasterLayersDataComponent,
    },
    render::eventually::Eventually,
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
                .push(RasterLayerData::Available(layer));
        }
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
                let source = layer.source().clone();
                if let Some(component) = world
                    .tiles
                    .query_mut::<&mut RasterLayersDataComponent>(coords)
                {
                    component.record(layer);
                }
                if let Some(Eventually::Initialized(raster)) =
                    world.resources.get_mut::<Eventually<RasterResources>>()
                {
                    raster.tile_data_changed(&source, coords);
                }
            }
        }
        Ok(())
    }
}

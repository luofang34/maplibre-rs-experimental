//! Raster source loading, cross-fade configuration, and tile-covering queries on [`HeadlessMap`].

use std::collections::BTreeMap;

use image::RgbaImage;

use super::{HeadlessMap, HeadlessMapOperationError, ProcessedLayers};
use crate::{
    coords::WorldTileCoords,
    raster::{
        cross_fade::RasterCrossFade, resource::RasterResources, AvailableRasterLayerData,
        MissingRasterLayerData, RasterLayerData, RasterLayersDataComponent, RasterSourceId,
    },
    render::{
        eventually::Eventually,
        projection::{raster_source_regions, ProjectionStateError},
        view_state::ViewStatePadding,
    },
    terrain::dem_store::{
        decode_image_to_dem, dem_unpack_for_source, ingest_source_dem, is_terrain_source,
    },
};

impl HeadlessMap {
    /// Renders already-decoded vector and raster source tiles in one frame.
    pub fn render_sources(
        &mut self,
        layers: ProcessedLayers,
        raster_layers: Vec<AvailableRasterLayerData>,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_source_frames(layers, raster_layers, 1)
    }

    /// Renders the same decoded source tiles for a fixed number of consecutive frames.
    pub fn render_source_frames(
        &mut self,
        layers: ProcessedLayers,
        raster_layers: Vec<AvailableRasterLayerData>,
        frame_count: u8,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_frames_with_terrain(layers, raster_layers, Vec::new(), frame_count)
    }

    /// Loads processed vector, raster and DEM data, then runs `frame_count` offscreen frames.
    /// A zero count is rejected before loading data. DEM decode failures use ancestor fallback;
    /// invalid tile coordinates and schedule failures are returned with their cause.
    pub fn render_frames_with_terrain(
        &mut self,
        layers: ProcessedLayers,
        raster_layers: Vec<AvailableRasterLayerData>,
        dem_tiles: Vec<(WorldTileCoords, RgbaImage)>,
        frame_count: u8,
    ) -> Result<(), HeadlessMapOperationError> {
        if frame_count == 0 {
            return Err(HeadlessMapOperationError::InvalidFrameCount);
        }
        if !dem_tiles.is_empty() {
            self.load_dem_tiles(dem_tiles)?;
        }
        let context = &mut self.map_context;
        layers.insert_into_tiles(&context.style, &mut context.world.tiles)?;

        self.load_raster_layers(raster_layers)?;
        self.advance_frames(frame_count)
    }

    /// Fades out raster tiles a camera change left behind, drawing them over the tiles that
    /// replace them as GL JS does during `raster-fade-duration`.
    pub fn set_raster_cross_fade(&mut self, fade: RasterCrossFade) {
        self.map_context.world.resources.insert(fade);
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
    }

    /// Returns the tiles a raster source must make available for this view: its own covering
    /// at its tile size and rounding, as the request system asks for them.
    pub fn required_raster_tile_coords(
        &self,
        source_name: &str,
    ) -> Result<Vec<WorldTileCoords>, ProjectionStateError> {
        let source = crate::io::tile_sources::source_layer_groups(
            &self.map_context.style,
            crate::io::tile_sources::TileKind::Raster,
        )
        .into_iter()
        .find(|group| {
            group
                .layers
                .iter()
                .any(|layer| layer.source.as_deref() == Some(source_name))
        })
        .map(|group| RasterSourceId::new(group.source_name));
        source.map_or_else(
            || Ok(Vec::new()),
            |source| self.required_raster_source_tile_coords(&source),
        )
    }

    /// Returns this source's visible tile coordinates, including the explicit unnamed fallback.
    pub fn required_raster_source_tile_coords(
        &self,
        source: &RasterSourceId,
    ) -> Result<Vec<WorldTileCoords>, ProjectionStateError> {
        let context = &self.map_context;
        Ok(raster_source_regions(
            &context.style,
            &context.view_state,
            &context.world,
            ViewStatePadding::Loose,
        )?
        .into_iter()
        .find(|(id, _)| id == source)
        .map_or_else(Vec::new, |(_, tiles)| tiles))
    }

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

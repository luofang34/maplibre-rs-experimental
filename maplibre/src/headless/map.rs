use std::{cell::RefCell, collections::BTreeMap, ops::Deref, rc::Rc};

use image::RgbaImage;
use thiserror::Error;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, TILE_SIZE},
    geojson::{process_geojson_features, GeoJsonTileRequest, ProcessGeoJsonError},
    headless::environment::HeadlessEnvironment,
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        source_client::SourceFetchError,
        source_type::{SourceType, TessellateSource},
    },
    kernel::Kernel,
    map::MapError,
    plugin::Plugin,
    raster::{AvailableRasterLayerData, RasterLayerData, RasterLayersDataComponent},
    render::{
        eventually::Eventually,
        projection::{view_region_for_projection, ProjectionStateError},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    schedule::{Schedule, Stage, StageError},
    style::{layer::StyleLayer, Style},
    tcs::world::World,
    terrain::{backfill_neighbours, dem::DemTile, source::dem_source, DemTileComponent, LoadedDem},
    vector::{
        process_vector_tile, AvailableVectorLayerBucket, DefaultVectorTransferables,
        LayerTessellated, ProcessVectorContext, ProcessVectorError, VectorBufferPool,
        VectorLayerBucket, VectorLayerBucketComponent, VectorTileRequest, VectorTransferables,
    },
};

/// Failure while processing or rendering data through a [`HeadlessMap`].
#[derive(Debug, Error)]
pub enum HeadlessMapOperationError {
    /// At least one frame is required for a render request.
    #[error("headless render frame count must be positive")]
    InvalidFrameCount,
    /// Tile coordinates cannot be represented by the tile store.
    #[error("cannot spawn headless tile {coords}")]
    InvalidTile {
        /// Invalid source-tile coordinates.
        coords: WorldTileCoords,
    },
    /// Vector source processing failed.
    #[error("headless vector source processing failed")]
    Vector {
        /// Underlying vector processor error.
        #[source]
        source: ProcessVectorError,
    },
    /// GeoJSON source processing failed.
    #[error("headless GeoJSON source processing failed")]
    GeoJson {
        /// Underlying GeoJSON processor error.
        #[source]
        source: ProcessGeoJsonError,
    },
    /// Render schedule execution failed.
    #[error("headless render schedule failed")]
    Schedule {
        /// Underlying schedule error.
        #[source]
        source: StageError,
    },
}

pub struct HeadlessMap {
    kernel: Rc<Kernel<HeadlessEnvironment>>,
    schedule: Schedule,
    map_context: MapContext,
}

impl HeadlessMap {
    pub fn new(
        style: Style,
        mut renderer: Renderer,
        kernel: Kernel<HeadlessEnvironment>,
        plugins: Vec<Box<dyn Plugin<HeadlessEnvironment>>>,
    ) -> Result<Self, MapError> {
        style.log_validation_errors();
        let window_size = renderer.state().surface().size();

        let view_state = initial_view_state(window_size, &style);

        let mut world = World::default();
        let mut schedule = Schedule::default();
        let kernel = Rc::new(kernel);

        for plugin in &plugins {
            plugin.build(
                &mut schedule,
                kernel.clone(),
                &mut world,
                &mut renderer.render_graph,
            );
        }

        Ok(Self {
            kernel,
            map_context: MapContext {
                style,
                view_state,
                world,
                renderer,
            },
            schedule,
        })
    }

    pub fn render_tile(
        &mut self,
        layers: Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_sources(layers, Vec::new())
    }

    /// Renders already-decoded vector and raster source tiles in one frame.
    pub fn render_sources(
        &mut self,
        layers: Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
        raster_layers: Vec<AvailableRasterLayerData>,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_source_frames(layers, raster_layers, 1)
    }

    /// Renders the same decoded source tiles for a fixed number of consecutive frames.
    pub fn render_source_frames(
        &mut self,
        layers: Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
        raster_layers: Vec<AvailableRasterLayerData>,
        frame_count: u8,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_frames_with_terrain(layers, raster_layers, Vec::new(), frame_count)
    }

    /// Renders decoded source tiles together with DEM tiles for the style's terrain.
    ///
    /// DEM images are decoded with the terrain source's encoding; images that cannot be decoded
    /// are skipped so the mesh falls back to an ancestor tile or sea level.
    /// Inserts decoded DEM tiles, fills the borders between neighbours and settles the terrain
    /// index with one schedule pass, so later tile selection sees the elevations they reveal.
    pub fn load_dem_tiles(
        &mut self,
        dem_tiles: Vec<(WorldTileCoords, RgbaImage)>,
    ) -> Result<(), HeadlessMapOperationError> {
        let context = &mut self.map_context;
        let unpack = dem_source(&context.style).map(|dem| dem.unpack);
        let tiles = &mut context.world.tiles;
        let mut dem_coords = Vec::new();
        for (coords, image) in dem_tiles {
            let Some(unpack) = unpack else {
                break;
            };
            let component = match DemTile::from_image(&image, unpack) {
                Ok(tile) => DemTileComponent::Loaded(LoadedDem::new(tile)),
                Err(error) => {
                    tracing::warn!(%coords, %error, "DEM tile image is unusable");
                    DemTileComponent::Missing
                }
            };
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(component);
            dem_coords.push(coords);
        }
        for coords in dem_coords {
            backfill_neighbours(tiles, coords);
        }
        if let Err(error) = self.schedule.run(context) {
            tracing::warn!(?error, "terrain index warm-up frame failed");
        }
        Ok(())
    }

    pub fn render_frames_with_terrain(
        &mut self,
        layers: Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
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
        let tiles = &mut context.world.tiles;

        let mut layers_by_tile = BTreeMap::new();
        for layer in layers {
            layers_by_tile
                .entry(layer.coords)
                .or_insert_with(Vec::new)
                .push(VectorLayerBucket::AvailableLayer(
                    AvailableVectorLayerBucket {
                        coords: layer.coords,
                        source_layer: layer.layer_data.name,
                        style_layer_id: layer.style_layer_id,
                        buffer: layer.buffer,
                        feature_indices: layer.feature_indices,
                        feature_colors: layer.feature_colors,
                    },
                ));
        }

        for (coords, layers) in layers_by_tile {
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(VectorLayerBucketComponent { done: true, layers });
        }

        let mut rasters_by_tile = BTreeMap::new();
        for layer in raster_layers {
            rasters_by_tile
                .entry(layer.coords)
                .or_insert_with(Vec::new)
                .push(RasterLayerData::Available(layer));
        }
        for (coords, layers) in rasters_by_tile {
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(RasterLayersDataComponent { layers });
        }

        for _ in 0..frame_count {
            self.schedule
                .run(context)
                .map_err(|source| HeadlessMapOperationError::Schedule { source })?;
        }

        let resources = &mut context.world.resources;
        let tiles = &mut context.world.tiles;

        tiles.clear();

        if let Some(Eventually::Initialized(pool)) =
            resources.query_mut::<&mut Eventually<VectorBufferPool>>()
        {
            pool.clear();
        }
        Ok(())
    }

    /// Raises the pitch limit and re-applies the style's pitch, which the default limit clamps.
    pub fn set_max_pitch(&mut self, max_pitch: cgmath::Deg<f64>) {
        let context = &mut self.map_context;
        context.view_state.set_max_pitch(max_pitch);
        let pitch = context.style.pitch.unwrap_or_default();
        context
            .view_state
            .camera_mut()
            .set_pitch(cgmath::Deg::<f64>(pitch));
    }

    /// Returns the tile coordinates the source pipeline must make available for this view.
    pub fn required_tile_coords(&self) -> Result<Vec<WorldTileCoords>, ProjectionStateError> {
        let context = &self.map_context;
        let visible_level = context.view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
        Ok(view_region_for_projection(
            &context.style,
            &context.view_state,
            &context.world,
            visible_level,
            ViewStatePadding::Loose,
        )?
        .map_or_else(Vec::new, |region| {
            region
                .iter()
                .filter(|coords| coords.build_quad_key().is_some())
                .collect()
        }))
    }

    pub async fn fetch_tile(&self, coords: WorldTileCoords) -> Result<Box<[u8]>, SourceFetchError> {
        let source_client = self.kernel.source_client();
        let data = source_client
            .fetch(
                &coords,
                &SourceType::Tessellate(TessellateSource::default()),
            )
            .await?
            .into_boxed_slice();
        Ok(data)
    }

    pub async fn process_tile(
        &self,
        tile_data: Box<[u8]>,
        layer: &StyleLayer,
    ) -> Result<
        Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
        HeadlessMapOperationError,
    > {
        self.process_tile_at(
            tile_data,
            layer,
            WorldTileCoords::default(),
            crate::projection::ProjectionType::Mercator,
        )
    }

    /// Processes one vector source tile with explicit coordinates and projection policy.
    pub fn process_tile_at(
        &self,
        tile_data: Box<[u8]>,
        layer: &StyleLayer,
        target_coords: WorldTileCoords,
        projection: crate::projection::ProjectionType,
    ) -> Result<
        Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
        HeadlessMapOperationError,
    > {
        let context = HeadlessContext::default();
        let mut processor =
            ProcessVectorContext::<DefaultVectorTransferables, HeadlessContext>::new(context);

        process_vector_tile(
            &tile_data,
            VectorTileRequest {
                coords: target_coords,
                layers: [layer].into_iter().cloned().collect(),
                projection,
            },
            &mut processor,
        )
        .map_err(|source| HeadlessMapOperationError::Vector { source })?;

        let messages = processor.take_context().messages.deref().take();
        let layers = messages.into_iter()
            .filter(|message| message.tag() == <DefaultVectorTransferables as VectorTransferables>::LayerTessellated::message_tag())
            .map(|message| message.into_transferable::<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>())
            .collect::<Vec<_>>();

        Ok(layers)
    }

    /// Process inline GeoJSON data for the given style layers and tile coordinates.
    ///
    /// Returns tessellated layers ready to be passed to [`Self::render_tile`].
    pub fn process_geojson(
        &mut self,
        geojson_value: &serde_json::Value,
        source_name: &str,
        matching_layers: Vec<StyleLayer>,
        target_coords: WorldTileCoords,
        projection: crate::projection::ProjectionType,
    ) -> Result<
        Vec<Box<<DefaultVectorTransferables as VectorTransferables>::LayerTessellated>>,
        HeadlessMapOperationError,
    > {
        let context = HeadlessContext::default();

        process_geojson_features::<DefaultVectorTransferables, HeadlessContext>(
            geojson_value,
            GeoJsonTileRequest {
                coords: target_coords,
                layers: matching_layers,
                source_name: source_name.to_owned(),
                projection,
            },
            &context,
        )
        .map_err(|source| HeadlessMapOperationError::GeoJson { source })?;

        let messages = context.messages.deref().take();
        Ok(messages
            .into_iter()
            .filter(|message| {
                message.tag()
                    == <DefaultVectorTransferables as VectorTransferables>::LayerTessellated::message_tag()
            })
            .map(|message| {
                message.into_transferable::<
                    <DefaultVectorTransferables as VectorTransferables>::LayerTessellated,
                >()
            })
            .collect())
    }
}

fn initial_view_state(window_size: crate::window::PhysicalSize, style: &Style) -> ViewState {
    let zoom = Zoom::new(style.zoom.unwrap_or_default());
    let center = style.center.map_or_else(
        || WorldCoords::from((TILE_SIZE / 2.0, TILE_SIZE / 2.0)),
        |center| WorldCoords::from_lat_lon(LatLon::new(center[1], center[0]), zoom),
    );
    let mut view_state = ViewState::new(
        window_size,
        center,
        zoom,
        cgmath::Deg(style.pitch.unwrap_or_default()),
        cgmath::Rad(0.6435011087932844),
    );
    view_state
        .camera_mut()
        .set_roll(cgmath::Deg(style.bearing.unwrap_or_default()));
    view_state
}

#[derive(Default, Clone)]
pub struct HeadlessContext {
    pub messages: Rc<RefCell<Vec<Message>>>,
}

impl Context for HeadlessContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.messages.deref().borrow_mut().push(message.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests;

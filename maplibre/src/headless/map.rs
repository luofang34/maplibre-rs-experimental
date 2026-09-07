use std::{cell::RefCell, collections::BTreeMap, ops::Deref, rc::Rc, time::Duration};

use image::RgbaImage;
use thiserror::Error;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, TILE_SIZE},
    geojson::ProcessGeoJsonError,
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
        frame_input::FrameInput,
        projection::{raster_source_regions, view_region_for_projection, ProjectionStateError},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    schedule::{Schedule, Stage, StageError},
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    style::{layer::StyleLayer, Style},
    tcs::world::World,
    terrain::{backfill_neighbours, dem::DemTile, source::dem_source, DemTileComponent, LoadedDem},
    vector::{
        transferables::SymbolLayerTessellated, AvailableVectorLayerBucket, ProcessVectorError,
        VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
    },
};

mod processed;
mod xr;

pub use xr::XrFrameError;

pub use processed::{
    process_geojson_layers, process_tile_layers, ProcessedLayers, SymbolLayer, VectorLayer,
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

/// A headless frame advances the frame clock by a nominal 60 Hz interval, so animated
/// properties progress the same way in every run.
const HEADLESS_FRAME_INTERVAL: Duration = Duration::from_millis(16);

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

    /// Renders processed vector and symbol layers in one frame.
    pub fn render_tile(
        &mut self,
        layers: ProcessedLayers,
    ) -> Result<(), HeadlessMapOperationError> {
        self.render_sources(layers, Vec::new())
    }

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
        let tiles = &mut context.world.tiles;
        let ProcessedLayers { vector, symbols } = layers;

        let mut layers_by_tile = BTreeMap::new();
        for layer in vector {
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

        let mut symbols_by_tile = BTreeMap::new();
        for layer in symbols {
            symbols_by_tile
                .entry(layer.coords())
                .or_insert_with(Vec::new)
                .push((*layer).to_bucket());
        }
        for (coords, layers) in symbols_by_tile {
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(SymbolLayersDataComponent { layers });
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
            context
                .world
                .resources
                .get_or_init_mut::<FrameInput>()
                .advance(HEADLESS_FRAME_INTERVAL);
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
        if let Some(Eventually::Initialized(pool)) =
            resources.query_mut::<&mut Eventually<SymbolBufferPool>>()
        {
            pool.clear();
        }
        Ok(())
    }

    /// Runs one frame of the schedule: requests, uploads and the render graph.
    ///
    /// A map built without the headless plugin keeps its request systems, so this is the
    /// frame step of a host that renders into the offscreen texture continuously.
    pub fn run_frame(&mut self) -> Result<(), HeadlessMapOperationError> {
        self.schedule
            .run(&mut self.map_context)
            .map_err(|source| HeadlessMapOperationError::Schedule { source })
    }

    /// The frame clock and view source the next frame applies.
    pub fn frame_input_mut(&mut self) -> &mut FrameInput {
        self.map_context
            .world
            .resources
            .get_or_init_mut::<FrameInput>()
    }

    /// The view the map renders from.
    /// The map's world, for tests that read what a frame left behind.
    #[cfg(test)]
    pub(crate) fn world(&self) -> &crate::tcs::world::World {
        &self.map_context.world
    }

    pub fn view_state(&self) -> &ViewState {
        &self.map_context.view_state
    }

    /// The texture the offscreen head renders into, in the surface format.
    pub fn head_texture(&self) -> Option<&wgpu::Texture> {
        match self.map_context.renderer.resources.surface.head() {
            crate::render::resource::Head::Headless(head) => Some(head.texture()),
            crate::render::resource::Head::Headed(_) => None,
        }
    }

    /// The device the renderer draws with.
    pub fn device(&self) -> &wgpu::Device {
        &self.map_context.renderer.device
    }

    /// The queue the renderer submits to.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.map_context.renderer.queue
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

    /// Returns the tiles a raster source must make available for this view: its own covering
    /// at its tile size and rounding, as the request system asks for them.
    pub fn required_raster_tile_coords(
        &self,
        source_name: &str,
    ) -> Result<Vec<WorldTileCoords>, ProjectionStateError> {
        let context = &self.map_context;
        Ok(raster_source_regions(
            &context.style,
            &context.view_state,
            &context.world,
            ViewStatePadding::Loose,
        )?
        .into_iter()
        .find(|(name, _)| name == source_name)
        .map_or_else(Vec::new, |(_, tiles)| tiles))
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

    /// Processes one vector source tile for a style layer at the origin tile in Mercator.
    pub async fn process_tile(
        &self,
        tile_data: Box<[u8]>,
        layer: &StyleLayer,
    ) -> Result<ProcessedLayers, HeadlessMapOperationError> {
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
    ) -> Result<ProcessedLayers, HeadlessMapOperationError> {
        process_tile_layers(&tile_data, layer, target_coords, projection)
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
    ) -> Result<ProcessedLayers, HeadlessMapOperationError> {
        process_geojson_layers(
            geojson_value,
            source_name,
            matching_layers,
            target_coords,
            projection,
        )
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
        .set_bearing(cgmath::Deg(style.bearing.unwrap_or_default()));
    view_state
        .camera_mut()
        .set_roll(cgmath::Deg(style.roll.unwrap_or_default()));
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

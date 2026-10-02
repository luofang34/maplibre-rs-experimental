//! Rendering, querying and readback for maps supplied with decoded source tiles.

use std::{cell::RefCell, collections::BTreeMap, ops::Deref, rc::Rc, time::Duration};

use image::RgbaImage;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom},
    headless::environment::HeadlessEnvironment,
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        source_client::SourceFetchError,
        source_type::{SourceType, TessellateSource},
    },
    kernel::Kernel,
    map::MapError,
    plugin::Plugin,
    raster::{AvailableRasterLayerData, RasterSourceId},
    render::{
        frame_input::FrameInput,
        projection::{raster_source_regions, view_region_for_projection, ProjectionStateError},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    schedule::{Schedule, Stage},
    sdf::SymbolLayersDataComponent,
    style::{layer::StyleLayer, Style},
    tcs::world::World,
    terrain::{backfill_neighbours, dem::DemTile, source::dem_source, DemTileComponent, LoadedDem},
    vector::{
        transferables::SymbolLayerTessellated, AvailableVectorLayerBucket, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

mod error;
pub use error::HeadlessMapOperationError;

mod processed;
mod raster;
pub mod reference;
mod symbols;
#[cfg(test)]
mod terrain_coverage;
mod xr;

pub use processed::{
    process_geojson_layers, process_geojson_layers_with_atlas, process_tile_layers,
    process_tile_layers_with_atlas, ProcessedLayers, SymbolLayer, VectorLayer,
};
pub use xr::XrFrameError;

/// A headless frame advances the frame clock by a nominal 60 Hz interval, so animated
/// properties progress the same way in every run.
const HEADLESS_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Owns offscreen frame state and runs the supplied plugins without a host event loop.
pub struct HeadlessMap {
    kernel: Rc<Kernel<HeadlessEnvironment>>,
    schedule: Schedule,
    map_context: MapContext,
}

impl HeadlessMap {
    /// Initializes the view from the style and builds plugins in their supplied order.
    /// The renderer and host kernel must already be initialized.
    pub fn new(
        mut style: Style,
        mut renderer: Renderer,
        kernel: Kernel<HeadlessEnvironment>,
        plugins: Vec<Box<dyn Plugin<HeadlessEnvironment>>>,
    ) -> Result<Self, MapError> {
        style.resolve_global_state();
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
                        feature_sort_keys: layer.feature_sort_keys,
                    },
                ));
        }

        for (coords, layers) in layers_by_tile {
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(VectorLayerBucketComponent {
                    done: true,
                    failed: false,
                    layers,
                    overscaled_zoom: 0,
                });
        }

        let mut symbols_by_tile = BTreeMap::new();
        for layer in symbols {
            symbols_by_tile
                .entry(layer.coords())
                .or_insert_with(Vec::new)
                .push((*layer).to_bucket());
        }
        // A tile whose only layers are symbols is still a delivered tile, as when a worker
        // reports it finished, or its labels would never be covered.
        for coords in symbols_by_tile.keys() {
            if tiles
                .query::<&VectorLayerBucketComponent>(*coords)
                .is_none()
            {
                tiles
                    .spawn_mut(*coords)
                    .ok_or(HeadlessMapOperationError::InvalidTile { coords: *coords })?
                    .insert(VectorLayerBucketComponent {
                        done: true,
                        failed: false,
                        layers: Vec::new(),
                        overscaled_zoom: 0,
                    });
            }
        }
        for (coords, layers) in symbols_by_tile {
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(SymbolLayersDataComponent {
                    layers,
                    pending_assets: false,
                });
        }

        self.load_raster_layers(raster_layers)?;
        self.advance_frames(frame_count)
    }

    fn advance_frames(&mut self, frame_count: u8) -> Result<(), HeadlessMapOperationError> {
        for _ in 0..frame_count {
            self.map_context
                .world
                .resources
                .get_or_init_mut::<FrameInput>()
                .advance(HEADLESS_FRAME_INTERVAL);
            self.schedule
                .run(&mut self.map_context)
                .map_err(|source| HeadlessMapOperationError::Schedule { source })?;
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

    /// Symbols accepted by placement under a point or box, narrowed by layers and a filter.
    pub fn query_rendered_symbols_in(
        &self,
        geometry: crate::sdf::query::QueryGeometry,
        options: &crate::sdf::query::QueryOptions,
    ) -> Result<Vec<crate::sdf::query::RenderedSymbol>, crate::sdf::query::QueryError> {
        crate::sdf::query::query_rendered_symbols_in(
            &self.map_context.world,
            &self.map_context.style,
            geometry,
            options,
        )
    }

    /// Symbols accepted by placement at a screen point, optionally restricted to style layers.
    pub fn query_rendered_symbols(
        &self,
        point: [f64; 2],
        layers: Option<&[&str]>,
    ) -> Vec<crate::sdf::query::RenderedSymbol> {
        crate::sdf::query::query_rendered_symbols(
            &self.map_context.world,
            &self.map_context.style,
            point,
            layers,
        )
    }

    /// The map's world, for tests that read what a frame left behind.
    #[cfg(test)]
    pub(crate) fn world(&self) -> &crate::tcs::world::World {
        &self.map_context.world
    }

    /// The view the map renders from.
    pub fn view_state(&self) -> &ViewState {
        &self.map_context.view_state
    }

    /// Mutable world access for tests that stage frame state.
    #[cfg(test)]
    pub(crate) fn world_mut(&mut self) -> &mut crate::tcs::world::World {
        &mut self.map_context.world
    }

    /// Moves the camera between frames, as a host gesture would.
    #[cfg(test)]
    pub(crate) fn view_state_mut(&mut self) -> &mut ViewState {
        &mut self.map_context.view_state
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

    /// Tells the map how much memory the host still has, or `None` when it cannot tell, so
    /// the frame takes nothing new on when little is left.
    pub fn set_available_memory(&mut self, available_bytes: Option<u64>) {
        let resources = &mut self.map_context.world.resources;
        let budget = resources
            .get_or_init_mut::<crate::render::memory_budget::MemoryBudgetTracker>()
            .update(available_bytes);
        resources.insert(budget);
    }

    /// Draws at `ratio` physical pixels per logical pixel: the view measures its viewport, and
    /// so its layout units, in logical pixels while the frame keeps its physical size.
    pub fn set_pixel_ratio(&mut self, ratio: f64) {
        let context = &mut self.map_context;
        let physical = context.renderer.state().surface().size();
        context.view_state.resize(physical.to_logical(ratio));
    }

    /// Pads the viewport, which moves the apparent center the camera looks through.
    pub fn set_padding(&mut self, padding: crate::render::camera::EdgeInsets) {
        self.map_context.view_state.set_edge_insets(padding);
    }

    /// Puts the camera's center at `meters` above sea level and holds it there, so terrain under
    /// the center does not move it.
    pub fn pin_center_elevation(&mut self, meters: f64) {
        let view_state = &mut self.map_context.view_state;
        view_state.set_center_elevation(meters);
        view_state.freeze_center_elevation();
    }

    /// Fades out raster tiles a camera change left behind, drawing them over the tiles that
    /// replace them as GL JS does during `raster-fade-duration`.
    pub fn set_raster_cross_fade(&mut self, fade: crate::raster::cross_fade::RasterCrossFade) {
        self.map_context.world.resources.insert(fade);
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

    /// Fetches a vector tile from [`TessellateSource::default`], independent of style sources.
    /// Addressing and HTTP failures preserve their underlying cause.
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
    // The flat world may not be smaller than the viewport is tall, as GL JS constrains the zoom;
    // a globe may be seen whole.
    let globe = style
        .projection
        .as_ref()
        .is_some_and(|specification| specification.projection_type.uses_globe_rendering(0.0));
    let lowest_zoom = if globe {
        f64::NEG_INFINITY
    } else {
        (f64::from(window_size.height()) / crate::coords::TILE_SIZE).log2()
    };
    let zoom = Zoom::new(style.zoom.unwrap_or_default().max(lowest_zoom));
    let center = style.center.unwrap_or_default();
    let mut center = WorldCoords::from_lat_lon(LatLon::new(center[1], center[0]), zoom);
    if !globe {
        // The view may not show past the poles of a flat world, as GL JS constrains the centre.
        let world = crate::coords::TILE_SIZE * 2_f64.powf(zoom.value());
        let half = f64::from(window_size.height()) / 2.0;
        if world > 2.0 * half {
            center.y = center.y.clamp(half, world - half);
        }
    }
    let mut view_state = ViewState::new(
        window_size,
        center,
        zoom,
        cgmath::Deg(style.pitch.unwrap_or_default()),
        style
            .vertical_field_of_view
            .map_or(cgmath::Rad(0.6435011087932844), |degrees| {
                cgmath::Rad::from(cgmath::Deg(degrees))
            }),
    );
    view_state
        .camera_mut()
        .set_bearing(cgmath::Deg(style.bearing.unwrap_or_default()));
    view_state
        .camera_mut()
        .set_roll(cgmath::Deg(style.roll.unwrap_or_default()));
    view_state
}

/// Collects processed worker messages locally for the offscreen map to consume.
#[derive(Default, Clone)]
pub struct HeadlessContext {
    /// Shared output queue; clones append to the same sequence of messages.
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

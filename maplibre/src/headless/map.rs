//! Rendering, querying and readback for maps supplied with decoded source tiles.

use std::{cell::RefCell, ops::Deref, rc::Rc, time::Duration};

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom},
    headless::environment::HeadlessEnvironment,
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        source_client::SourceFetchError,
        source_type::{SourceType, TessellateSource},
    },
    kernel::Kernel,
    map::MapError,
    plugin::Plugin,
    render::{
        frame_input::FrameInput,
        projection::{view_region_for_projection, ProjectionStateError},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    schedule::Schedule,
    style::{layer::StyleLayer, Style},
    tcs::world::World,
};

mod camera;
mod dem;
mod error;
#[cfg(test)]
mod injected_loader;
mod labels;
pub mod multi_view;
pub use error::HeadlessMapOperationError;
#[cfg(feature = "experimental")]
mod experimental;
mod processed;
mod provided_images;
#[cfg(feature = "raster")]
mod raster;
pub mod readback;
pub mod reference;
mod signals;
mod style;
#[cfg(test)]
mod symbol_worker;
mod symbols;
mod targets;
#[cfg(test)]
mod terrain_coverage;
mod terrain_patch;
#[cfg(test)]
mod test_async;
#[cfg(all(test, feature = "experimental"))]
mod vertical_perspective;
mod xr;

pub use multi_view::{
    CoveringPolicy, MultiViewError, MultiViewFrame, SlotView, ViewSlotConfig, ViewSlotEntry,
    ViewSlotId,
};
pub use processed::{
    process_geojson_layers, process_geojson_layers_with_atlas, process_tile_layers,
    process_tile_layers_for_style, process_tile_layers_with_atlas, ProcessedLayers, SymbolLayer,
    VectorLayer,
};
pub use terrain_patch::TerrainPatchSnapshot;
pub use xr::XrFrameError;

pub use crate::io::tile_json::resolve_tile_json_sources;

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
    ///
    /// Sources that name a TileJSON `url` draw nothing until it is resolved; await
    /// [`resolve_tile_json_sources`] with the kernel's [`Kernel::source_client`] first. It needs
    /// no blocking executor, so a browser host awaits it in its async startup.
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
            plugin
                .build(&mut schedule, kernel.clone(), &mut world, &mut renderer)
                .map_err(MapError::Plugin)?;
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
        let context = &mut self.map_context;
        layers.insert_into_tiles(&context.style, &mut context.world.tiles)?;
        self.advance_frames(1)
    }

    pub(super) fn advance_frames(
        &mut self,
        frame_count: u8,
    ) -> Result<(), HeadlessMapOperationError> {
        for _ in 0..frame_count {
            self.map_context
                .world
                .resources
                .get_or_init_mut::<FrameInput>()
                .advance(HEADLESS_FRAME_INTERVAL);
            crate::render::frame::run_frame(&mut self.schedule, &mut self.map_context)
                .map_err(|source| HeadlessMapOperationError::Schedule { source })?;
        }
        Ok(())
    }

    /// Runs one frame of the schedule: requests, uploads and the render graph.
    ///
    /// A map built without the headless plugin keeps its request systems, so this is the
    /// frame step of a host that renders into the offscreen texture continuously.
    pub fn run_frame(&mut self) -> Result<(), HeadlessMapOperationError> {
        crate::render::frame::run_frame(&mut self.schedule, &mut self.map_context)
            .map_err(|source| HeadlessMapOperationError::Schedule { source })
    }

    /// The frame clock and view source the next frame applies.
    pub fn frame_input_mut(&mut self) -> &mut FrameInput {
        self.map_context
            .world
            .resources
            .get_or_init_mut::<FrameInput>()
    }

    /// Fill, line and symbol features under a point or box, topmost first, as GL JS
    /// `queryRenderedFeatures` returns them.
    pub fn query_rendered_features(
        &self,
        geometry: crate::sdf::query::QueryGeometry,
        options: &crate::sdf::query::QueryOptions,
    ) -> Result<Vec<crate::query::QueriedFeature>, crate::sdf::query::QueryError> {
        self.map_context.query_rendered_features(geometry, options)
    }

    /// The map's world, for tests that read what a frame left behind.
    #[cfg(test)]
    pub(crate) fn world(&self) -> &crate::tcs::world::World {
        &self.map_context.world
    }

    /// The map's frame state, for what a host reads of it, such as the ground under a place
    /// ([`MapContext::loaded_ground_cached`], [`MapContext::drawn_ground_cached`]).
    pub fn context(&self) -> &MapContext {
        &self.map_context
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

    /// The renderer, for tests that look at its attachments.
    #[cfg(test)]
    pub(crate) fn renderer(&self) -> &crate::render::Renderer {
        &self.map_context.renderer
    }

    /// The queue the map's own uploads are counted through.
    #[cfg(test)]
    pub(crate) fn upload_queue(&self) -> &crate::render::upload_queue::UploadQueue {
        &self.map_context.renderer.queue
    }

    /// Moves the camera between frames, as a host gesture would.
    #[cfg(test)]
    pub(crate) fn view_state_mut(&mut self) -> &mut ViewState {
        &mut self.map_context.view_state
    }

    /// The device the renderer draws with.
    pub fn device(&self) -> &wgpu::Device {
        &self.map_context.renderer.device
    }

    /// The queue the renderer submits to. What a host writes through it is its own upload,
    /// not counted in the map's frame statistics.
    pub fn queue(&self) -> &wgpu::Queue {
        self.map_context.renderer.queue.inner()
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
        crate::sdf::provided::set_pixel_ratio(&mut context.world, ratio);
    }

    /// Pads the viewport, which moves the apparent center the camera looks through.
    pub fn set_padding(&mut self, padding: crate::render::camera::EdgeInsets) {
        self.map_context.view_state.set_edge_insets(padding);
    }

    /// What the render passes recorded since the last call: draws, and the state changes they
    /// recorded or skipped as already bound.
    pub fn take_render_stats(&self) -> crate::render::tracked_pass::PassStats {
        self.map_context
            .world
            .resources
            .get::<crate::render::tracked_pass::RenderStats>()
            .map(crate::render::tracked_pass::RenderStats::take)
            .unwrap_or_default()
    }

    /// Whether terrain tiles hang skirts below their edges, as GL JS `terrainSkirtLength`.
    pub fn set_terrain_skirts(&mut self, skirts: crate::terrain::TerrainSkirts) {
        self.map_context.world.resources.insert(skirts);
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
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
    let mut center =
        WorldCoords::from_lat_lon(crate::map::camera::clamped_style_center(style), zoom);
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
        cgmath::Rad(0.6435011087932844),
    );
    view_state.take_style_center(style);
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

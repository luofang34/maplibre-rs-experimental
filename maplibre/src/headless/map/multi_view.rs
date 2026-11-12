//! Heterogeneous multi-viewport rendering (`MultiViewFrame`) over a single [`HeadlessMap`].
//!
//! Registering lightweight [`ViewSlotId`]s on one [`HeadlessMap`] allows independent cameras
//! and resolutions (such as a `1920×1080` forward OTW camera at `60–120 Hz` alongside a
//! `640×480` nadir/oblique sensor camera at `30 Hz`, or a `ConformalSvs3D` PFD alongside a
//! `Map2D` MFD) to share a single `World`, `VectorBufferPool`, `DrapeScratch`, `DrapeCache`,
//! and compiled pipeline set without tile or drape thrashing.

use std::{collections::BTreeMap, time::Duration};

use rustc_hash::FxHashSet;
use thiserror::Error;

mod covering;
mod execute;

use covering::StereoSnapshot;

pub use crate::render::eye_covering::ViewSlotId;
use crate::{
    coords::{LatLon, WorldTileCoords, Zoom},
    headless::map::HeadlessMap,
    render::{
        eventually::Eventually,
        eye_covering::FrameLodHistory,
        resource::{Head, Surface, Texture},
        view_state::{ExternalView, ExternalViewError, NavigationError},
        Renderer,
    },
    schedule::StageError,
    sdf::SymbolDepth,
    tcs::world::World,
    window::PhysicalSize,
};
#[cfg(feature = "experimental")]
use crate::{
    overlay::{DepthProbe, HostOverlay, OverlayGpu, OverlayMesh, ProbeGpu, ProbeResults},
    starfield::sun::depth::SunDepth,
};

/// Attachment configuration for a registered [`ViewSlotId`].
#[derive(Clone, Copy, Debug)]
pub struct ViewSlotConfig {
    /// Physical pixel dimensions of this viewport's color and depth attachments.
    pub size: PhysicalSize,
    /// Whether to resolve a persistent 1× `Depth32Float` readout texture via `DEPTH_COPY`.
    pub depth_readout: bool,
}

/// How a slot selects its tile covering during [`HeadlessMap::run_multi_view_frame`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoveringPolicy {
    /// Reuses the tile covering and terrain surface selection prepared for an earlier
    /// [`CoveringPolicy::Independent`] slot in the same frame (zero-overhead stereo pair).
    StereoReuse(ViewSlotId),
    /// Computes this slot's own visible covering and unions its requests/residency with all
    /// other registered slots.
    Independent,
}

/// Camera pose rendered by a [`ViewSlotEntry`].
#[derive(Clone, Copy, Debug)]
pub enum SlotView {
    /// Host-driven 6-DoF external camera / sensor frustum.
    External(ExternalView),
    /// Map camera specified by geographic center, zoom, bearing, and pitch.
    Camera {
        /// Geographic center of the viewport.
        center: LatLon,
        /// Map zoom level.
        zoom: Zoom,
        /// Camera bearing in degrees clockwise from north.
        bearing_deg: f64,
        /// Camera pitch in degrees from nadir.
        pitch_deg: f64,
    },
}

/// One active viewport rendered in a [`MultiViewFrame`].
///
/// Registered slots omitted from [`MultiViewFrame::slots`] are skipped for that frame without
/// evicting their resident tiles or baked terrain drapes.
#[derive(Clone, Copy, Debug)]
pub struct ViewSlotEntry {
    /// Target registered slot.
    pub slot: ViewSlotId,
    /// Camera or external eye pose for this slot on this frame.
    pub view: SlotView,
    /// Covering policy (`Independent` or `StereoReuse(primary)`).
    pub policy: CoveringPolicy,
}

/// Frame specification rendering one or more active [`ViewSlotId`]s over one [`HeadlessMap`].
#[derive(Clone, Debug)]
pub struct MultiViewFrame {
    /// Host frame timestamp used by animated paint properties and motion prefetch.
    pub timestamp: Duration,
    /// Whether globe sky/background draws an opaque environment behind the body.
    pub opaque_environment: bool,
    /// Frustum tangent overscan factor (`>= 1.0`) for external-eye tile prefetch.
    pub request_overscan: f64,
    /// Active slots to draw in order; omitted registered slots are skipped this frame.
    pub slots: Vec<ViewSlotEntry>,
}

impl Default for MultiViewFrame {
    fn default() -> Self {
        Self {
            timestamp: Duration::ZERO,
            opaque_environment: true,
            request_overscan: 1.0,
            slots: Vec::new(),
        }
    }
}

/// Failure while validating or rendering a [`MultiViewFrame`].
#[derive(Debug, Error)]
pub enum MultiViewError {
    /// Referenced a [`ViewSlotId`] that is not registered on this map.
    #[error("view slot {slot:?} is not registered")]
    UnknownSlot {
        /// Unregistered slot identifier.
        slot: ViewSlotId,
    },
    /// `CoveringPolicy::StereoReuse(primary)` referenced a primary slot that did not precede it
    /// as an active `Independent` slot in the same frame.
    #[error("stereo slot {slot:?} references primary {primary:?} which was not drawn earlier as Independent")]
    InvalidStereoPrimary {
        /// Dependent stereo slot.
        slot: ViewSlotId,
        /// Referenced primary slot.
        primary: ViewSlotId,
    },
    /// External view validation failed for an active slot.
    #[error("invalid external view on slot")]
    InvalidExternalView {
        /// Underlying external-view error.
        #[source]
        source: ExternalViewError,
    },
    /// Map camera validation failed for an active slot.
    #[error("invalid camera options on slot")]
    InvalidCamera {
        /// Underlying navigation error.
        #[source]
        source: NavigationError,
    },
    /// A render schedule stage failed while executing a frame or slot.
    #[error("stage failed for slot {slot:?}")]
    SlotStage {
        /// Slot executing when the stage failed, or `None` for shared `Extract`/`Cleanup`.
        slot: Option<ViewSlotId>,
        /// Underlying schedule stage failure.
        #[source]
        source: StageError,
    },
}

struct SlotState {
    surface: Surface,
    depth_readout: bool,
    depth_texture: Eventually<Texture>,
    depth_readout_texture: Eventually<Texture>,
    multisampling_texture: Eventually<Option<Texture>>,
    symbol_depth: Eventually<SymbolDepth>,
    #[cfg(feature = "experimental")]
    sun_depth: Eventually<SunDepth>,
    #[cfg(feature = "experimental")]
    overlay: HostOverlay,
    #[cfg(feature = "experimental")]
    overlay_gpu: Eventually<OverlayGpu>,
    #[cfg(feature = "experimental")]
    probe_gpu: Eventually<ProbeGpu>,
    edge_cache: crate::terrain::queue_system::edges::EdgeCache,
    lod_history: FrameLodHistory,
    stereo_snapshot: Option<StereoSnapshot>,
    prepared_terrain: Option<crate::terrain::coverage::PreparedTerrainRegion>,
    frame_loose_in_use: FxHashSet<WorldTileCoords>,
    last_retained_tiles: FxHashSet<WorldTileCoords>,
    last_retained_drapes: FxHashSet<WorldTileCoords>,
    last_wanted_vector: FxHashSet<WorldTileCoords>,
    last_wanted_raster: FxHashSet<WorldTileCoords>,
    last_wanted_dem: FxHashSet<WorldTileCoords>,
    last_drawn_count: usize,
    /// See [`HeadlessMap::set_view_slot_drape_priority`].
    drape_priority: i32,
}

#[derive(Default)]
struct MultiViewRegistry {
    next_id: u32,
    frame_counter: u64,
    slots: BTreeMap<ViewSlotId, SlotState>,
}

fn swap_slot_resources(slot: &mut SlotState, renderer: &mut Renderer, world: &mut World) {
    std::mem::swap(&mut slot.surface, &mut renderer.resources.surface);
    std::mem::swap(
        &mut slot.depth_readout,
        &mut renderer.settings.depth_readout,
    );
    std::mem::swap(
        &mut slot.depth_texture,
        &mut renderer.resources.depth_texture,
    );
    std::mem::swap(
        &mut slot.depth_readout_texture,
        &mut renderer.resources.depth_readout_texture,
    );
    std::mem::swap(
        &mut slot.multisampling_texture,
        &mut renderer.resources.multisampling_texture,
    );
    if let Some(symbol_depth) = world.resources.get_mut::<Eventually<SymbolDepth>>() {
        std::mem::swap(&mut slot.symbol_depth, symbol_depth);
    }
    #[cfg(feature = "experimental")]
    if let Some(sun_depth) = world.resources.get_mut::<Eventually<SunDepth>>() {
        std::mem::swap(&mut slot.sun_depth, sun_depth);
    }
    #[cfg(feature = "experimental")]
    if let Some(overlay) = world.resources.get_mut::<HostOverlay>() {
        std::mem::swap(&mut slot.overlay, overlay);
    }
    #[cfg(feature = "experimental")]
    if let Some(overlay_gpu) = world.resources.get_mut::<Eventually<OverlayGpu>>() {
        std::mem::swap(&mut slot.overlay_gpu, overlay_gpu);
    }
    #[cfg(feature = "experimental")]
    if let Some(probe_gpu) = world.resources.get_mut::<Eventually<ProbeGpu>>() {
        std::mem::swap(&mut slot.probe_gpu, probe_gpu);
    }
    std::mem::swap(
        &mut slot.edge_cache,
        world
            .resources
            .get_or_init_mut::<crate::terrain::queue_system::edges::EdgeCache>(),
    );
}

impl HeadlessMap {
    /// Registers a new lightweight viewport slot owning its own `(width, height)` attachments,
    /// LOD history, and optional overlay / depth-probe state.
    pub fn register_view_slot(&mut self, config: ViewSlotConfig) -> ViewSlotId {
        let surface = Surface::from_headless_size(
            &self.map_context.renderer.device,
            &self.map_context.renderer.adapter,
            config.size,
            &self.map_context.renderer.settings,
        );
        let registry = self
            .map_context
            .world
            .resources
            .get_or_init_mut::<MultiViewRegistry>();
        let id = ViewSlotId::from_raw(registry.next_id);
        registry.next_id = registry.next_id.wrapping_add(1);
        registry.slots.insert(
            id,
            SlotState {
                surface,
                depth_readout: config.depth_readout,
                depth_texture: Eventually::Uninitialized,
                depth_readout_texture: Eventually::Uninitialized,
                multisampling_texture: Eventually::Uninitialized,
                symbol_depth: Eventually::Uninitialized,
                #[cfg(feature = "experimental")]
                sun_depth: Eventually::Uninitialized,
                #[cfg(feature = "experimental")]
                overlay: HostOverlay::default(),
                #[cfg(feature = "experimental")]
                overlay_gpu: Eventually::Uninitialized,
                #[cfg(feature = "experimental")]
                probe_gpu: Eventually::Uninitialized,
                edge_cache: crate::terrain::queue_system::edges::EdgeCache::default(),
                lod_history: FrameLodHistory::default(),
                stereo_snapshot: None,
                prepared_terrain: None,
                frame_loose_in_use: FxHashSet::default(),
                last_retained_tiles: FxHashSet::default(),
                last_retained_drapes: FxHashSet::default(),
                last_wanted_vector: FxHashSet::default(),
                last_wanted_raster: FxHashSet::default(),
                last_wanted_dem: FxHashSet::default(),
                last_drawn_count: 0,
                drape_priority: 0,
            },
        );
        id
    }

    /// Resizes a registered view slot; GPU attachments reconfigure lazily on its next active frame.
    pub fn resize_view_slot(
        &mut self,
        slot: ViewSlotId,
        size: PhysicalSize,
    ) -> Result<(), MultiViewError> {
        let state = self.slot_mut(slot)?;
        state.surface.resize(size);
        Ok(())
    }

    /// Ranks `slot` among the views for the terrain drape textures memory allows the map.
    ///
    /// Every view holds its own drapes, and the map's allowance of drape textures is granted
    /// to the views at the start of each frame, by need, the highest priority first and slots
    /// of equal priority in the order they were registered. The first view drapes its ground
    /// at the detail it would have alone; each view after it has the textures that are left,
    /// never fewer than a small floor, and covers its ground with fewer, coarser drapes when
    /// they are short. Every slot starts at priority `0`, so the slot registered first is
    /// served first until the host says otherwise.
    pub fn set_view_slot_drape_priority(
        &mut self,
        slot: ViewSlotId,
        priority: i32,
    ) -> Result<(), MultiViewError> {
        self.slot_mut(slot)?.drape_priority = priority;
        Ok(())
    }

    /// Enables or disables the 1× `Depth32Float` readout attachment for `slot`.
    pub fn set_view_slot_depth_readout(
        &mut self,
        slot: ViewSlotId,
        enabled: bool,
    ) -> Result<(), MultiViewError> {
        let state = self.slot_mut(slot)?;
        state.depth_readout = enabled;
        Ok(())
    }

    /// Shows or hides the navigation HUD ([`crate::hud::NavigationHudPlugin`]) in `slot`.
    ///
    /// The HUD belongs to the map's own view, whose camera its controls steer, so a slot
    /// draws none until asked to. A slot that shows it lays it out for its own size; pointer
    /// input is still hit-tested against the map's own viewport.
    pub fn set_view_slot_navigation_hud(
        &mut self,
        slot: ViewSlotId,
        shown: bool,
    ) -> Result<(), MultiViewError> {
        self.slot_mut(slot)?;
        let resources = &mut self.map_context.world.resources;
        resources
            .get_or_init_mut::<crate::hud::HudViewSlots>()
            .set(slot, shown);
        Ok(())
    }

    /// Unregisters `slot` and releases its per-slot GPU attachments and terrain drapes.
    pub fn remove_view_slot(&mut self, slot: ViewSlotId) -> bool {
        let resources = &mut self.map_context.world.resources;
        let removed = resources
            .get_mut::<MultiViewRegistry>()
            .is_some_and(|registry| registry.slots.remove(&slot).is_some());
        if removed {
            if let Some(hud) = resources.get_mut::<crate::hud::HudViewSlots>() {
                hud.set(slot, false);
            }
            if let Some(Eventually::Initialized(terrain)) =
                resources.get_mut::<Eventually<crate::terrain::resources::TerrainResources>>()
            {
                terrain.release_drapes_of(Some(slot));
            }
        }
        removed
    }

    /// Replaces the host [`OverlayMesh`] drawn inside `slot` (`OverlayPlugin` required).
    #[cfg(feature = "experimental")]
    pub fn set_view_slot_overlay(
        &mut self,
        slot: ViewSlotId,
        mesh: OverlayMesh,
    ) -> Result<(), MultiViewError> {
        self.slot_mut(slot)?.overlay.set(mesh);
        Ok(())
    }

    /// Submits [`DepthProbe`]s to be tested against `slot`'s depth buffer (`OverlayPlugin` required).
    #[cfg(feature = "experimental")]
    pub fn set_view_slot_depth_probes(
        &mut self,
        slot: ViewSlotId,
        probes: Vec<DepthProbe>,
    ) -> Result<u64, MultiViewError> {
        Ok(self.slot_mut(slot)?.overlay.set_probes(probes))
    }

    /// Takes the latest completed [`ProbeResults`] for `slot`, if ready.
    #[cfg(feature = "experimental")]
    pub fn take_view_slot_depth_probe_results(
        &mut self,
        slot: ViewSlotId,
    ) -> Result<Option<ProbeResults>, MultiViewError> {
        Ok(self.slot_mut(slot)?.overlay.take_answers())
    }

    /// Borrows `slot`'s resolved single-sample color [`wgpu::Texture`].
    pub fn view_slot_color_texture(&self, slot: ViewSlotId) -> Option<&wgpu::Texture> {
        match self.slot_ref(slot)?.surface.head() {
            Head::Headless(head) => Some(head.texture()),
            Head::Headed(_) => None,
        }
    }

    /// Borrows `slot`'s resolved single-sample color [`wgpu::TextureView`].
    pub fn view_slot_color_view(&self, slot: ViewSlotId) -> Option<&wgpu::TextureView> {
        match self.slot_ref(slot)?.surface.head() {
            Head::Headless(head) => Some(head.view()),
            Head::Headed(_) => None,
        }
    }

    /// Borrows `slot`'s resolved 1× `Depth32Float` [`wgpu::Texture`], once rendered with
    /// `depth_readout: true`.
    pub fn view_slot_depth_texture(&self, slot: ViewSlotId) -> Option<&wgpu::Texture> {
        match &self.slot_ref(slot)?.depth_readout_texture {
            Eventually::Initialized(tex) => Some(&tex.texture),
            Eventually::Uninitialized => None,
        }
    }

    /// Borrows `slot`'s resolved 1× `Depth32Float` [`wgpu::TextureView`], once rendered with
    /// `depth_readout: true`.
    pub fn view_slot_depth_view(&self, slot: ViewSlotId) -> Option<&wgpu::TextureView> {
        match &self.slot_ref(slot)?.depth_readout_texture {
            Eventually::Initialized(tex) => Some(&tex.view),
            Eventually::Uninitialized => None,
        }
    }

    /// Whether `slot`'s offscreen head has allocated a CPU `MAP_READ` staging buffer.
    pub fn view_slot_has_readback_buffer(&self, slot: ViewSlotId) -> Option<bool> {
        match self.slot_ref(slot)?.surface.head() {
            Head::Headless(head) => Some(head.has_readback_buffer()),
            Head::Headed(_) => None,
        }
    }

    fn slot_ref(&self, slot: ViewSlotId) -> Option<&SlotState> {
        self.map_context
            .world
            .resources
            .get::<MultiViewRegistry>()?
            .slots
            .get(&slot)
    }

    fn slot_mut(&mut self, slot: ViewSlotId) -> Result<&mut SlotState, MultiViewError> {
        self.map_context
            .world
            .resources
            .get_mut::<MultiViewRegistry>()
            .and_then(|registry| registry.slots.get_mut(&slot))
            .ok_or(MultiViewError::UnknownSlot { slot })
    }
}

#[cfg(test)]
mod hud_tests;
#[cfg(all(test, feature = "experimental"))]
mod shared_assets_tests;
#[cfg(all(test, feature = "experimental"))]
mod streaming_tests;
#[cfg(all(test, feature = "experimental"))]
mod tests;
#[cfg(all(test, feature = "experimental"))]
mod view_independence_tests;

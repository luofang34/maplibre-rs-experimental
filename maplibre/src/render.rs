//! This module implements the rendering algorithm of maplibre-rs. It manages the whole
//! communication with the GPU.
//!
//! The render in this module is largely based on the
//! [bevy_render](https://github.com/bevyengine/bevy/tree/aced6a/crates/bevy_render)
//! crate with commit `aced6a`.
//! It is dual-licensed under MIT and Apache:
//!
//! ```text
//! Bevy is dual-licensed under either
//!
//! * MIT License (docs/LICENSE-MIT or http://opensource.org/licenses/MIT)
//! * Apache License, Version 2.0 (docs/LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
//!
//! at your option.
//! ```
//!
//! We appreciate the design and implementation work which as gone into it.
//!

#![forbid(unsafe_code)]

use std::{ops::Deref, rc::Rc, sync::Arc};

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        error::RenderError,
        eventually::Eventually,
        graph::{EmptyNode, RenderGraph, RenderGraphError},
        main_pass::MainPassNode,
        resource::{share_gpu, Head, Surface, Texture, TextureView},
        settings::{RendererSettings, WgpuSettings},
        systems::{
            cleanup_system::cleanup_system, resource_system::ResourceSystem,
            retention_system::retention_system, sort_phase_system::sort_phase_system,
            tile_view_pattern_system::tile_view_pattern_system,
        },
    },
    schedule::{Schedule, StageLabel},
    tcs::{
        system::{stage::SystemStage, SystemContainer},
        world::World,
    },
    window::{HeadedMapWindow, MapWindow},
};

pub(crate) mod depth_copy;
pub mod graph;
pub mod resource;
pub(crate) mod systems;

mod graph_runner;
mod main_pass;
pub(crate) use main_pass::{EMPTY_STENCIL_REFERENCE, STENCIL_MARK};
pub mod shaders;
mod translucent_pass;

pub mod builder;
pub mod camera;
pub mod error;
pub mod eventually;
pub mod eye_covering;
pub mod memory_budget;
#[cfg(all(test, feature = "headless"))]
pub(crate) use systems::retention_system::drawn_tiles;
#[cfg(feature = "headless")]
pub(crate) use systems::retention_system::RetainLoadedTiles;
pub mod after_submit;
pub mod frame;
pub mod frame_input;
pub mod frame_signals;
pub mod frame_trace;
pub mod gpu_timer;
pub mod ground;
#[cfg(feature = "headless")]
pub mod host_gpu;
pub mod pass_hooks;
pub mod placement_3d;
pub mod projection;
pub mod render_commands;
pub mod render_phase;
pub mod resolved_view;
pub mod scene_lighting;
pub mod settings;
pub mod tile_mesh;
pub mod tile_view_pattern;
pub mod tracked_pass;
pub mod upload_queue;
pub mod view_state;
pub mod xr;

pub use after_submit::*;
pub use pass_hooks::*;
pub use resolved_view::{ProjectedWindowPoint, ProjectionMode, ResolvedFrameView};
pub use scene_lighting::*;
pub use shaders::ShaderVertex;

use crate::{
    render::{
        depth_copy::{DepthCopyNode, DepthCopyPipeline},
        render_phase::{LayerItem, RenderPhase, TileMaskItem, TranslucentItem},
        systems::{graph_runner_system::GraphRunnerSystem, upload_system::upload_system},
        tile_view_pattern::{ViewTileSources, WgpuTileViewPattern},
        translucent_pass::TranslucentPassNode,
    },
    window::PhysicalSize,
};

pub(crate) const INDEX_FORMAT: wgpu::IndexFormat = wgpu::IndexFormat::Uint32; // Must match IndexDataType

/// The labels of the default App rendering stages.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum RenderStageLabel {
    /// Applies the host's frame input and advances camera animation, so every later system
    /// sees the frame's view.
    Input,

    /// Requests the tiles the view needs and applies the worker results that arrived. A map
    /// that is handed its tiles, such as a headless one, has no request stage.
    Request,

    /// Creates or resizes GPU resources, such as pipelines and frame attachments, and writes
    /// the frame's uniforms.
    Prepare,

    /// Uploads tile data and queues the [`PhaseItems`](render_phase::PhaseItem) the
    /// [`Render`](RenderStageLabel::Render) stage draws.
    Queue,

    /// Sorts the [`RenderPhases`](crate::render::render_phase::RenderPhase) and reads GPU
    /// timings that came back from earlier frames.
    PhaseSort,

    /// Records, submits and presents the frame.
    Render,

    /// Clears the frame's render phases and evicts tiles that left the view; it runs even when
    /// an earlier stage failed.
    Cleanup,
}

impl StageLabel for RenderStageLabel {
    fn dyn_clone(&self) -> Box<dyn StageLabel> {
        Box::new(self.clone())
    }
}

/// Frame attachments and presentation storage prepared by the render resource stage.
#[deny(missing_docs)]
pub struct RenderResources {
    /// Presentation or offscreen storage, including requested dimensions.
    pub surface: Surface,
    /// Acquired frame view; presentation consumes a window frame and releases this slot.
    pub render_target: Eventually<TextureView>,
    /// Depth/stencil attachment recreated when the target dimensions change.
    pub depth_texture: Eventually<Texture>,
    /// Optional multisample color attachment; initialized `None` means single-sample rendering.
    pub multisampling_texture: Eventually<Option<Texture>>,
    /// A host's `Depth32Float` texture the frame's depth is copied into after everything is
    /// drawn, for a compositor that reprojects the frame.
    pub eye_depth_target: Option<wgpu::TextureView>,
    /// Persistent single-sample `Depth32Float` texture populated via `DEPTH_COPY` when
    /// [`RendererSettings::depth_readout`] is enabled on an offscreen target.
    pub depth_readout_texture: Eventually<Texture>,
}

#[deny(missing_docs)]
impl RenderResources {
    /// Owns the surface with dependent attachments left uninitialized for the prepare stage.
    pub fn new(surface: Surface) -> Self {
        Self {
            render_target: Default::default(),
            depth_texture: Default::default(),
            multisampling_texture: Default::default(),
            eye_depth_target: None,
            depth_readout_texture: Default::default(),
            surface,
        }
    }

    /// Recreates a resized window surface, propagating host-handle and creation failures.
    /// Offscreen and unchanged window surfaces are untouched; dependent attachments are not rebuilt.
    /// The recreated surface retains its own cloned window-handle owner.
    pub fn recreate_surface<MW>(
        &mut self,
        window: &MW,
        instance: &wgpu::Instance,
    ) -> Result<(), RenderError>
    where
        MW: MapWindow + HeadedMapWindow,
    {
        self.surface.recreate::<MW>(window, instance)
    }

    /// Borrows the presentation or offscreen storage, including any pending resize.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }
}

/// GPU device, frame resources and graph used by the rendering schedule.
/// Construction leaves the graph empty; plugins install the passes and scheduled systems.
#[deny(missing_docs)]
pub struct Renderer {
    /// Backend instance from which the adapter and presentation surfaces are created.
    pub instance: wgpu::Instance,
    /// Shared device ownership allows offscreen capture to retain it while a readback is pending.
    pub device: Arc<wgpu::Device>,
    /// Queue for buffer uploads and encoded frame submissions, counting the bytes uploaded.
    pub queue: upload_queue::UploadQueue,
    /// Selected adapter, used to inspect backend capabilities and format support.
    pub adapter: wgpu::Adapter,

    /// Requested device configuration; actual enabled capabilities are available from the device.
    pub wgpu_settings: WgpuSettings,
    /// Render settings adjusted for backend depth and MSAA support during initialization.
    /// Changing these fields does not automatically rebuild existing pipelines.
    pub settings: RendererSettings,

    /// Presentation state and attachments shared by graph nodes.
    pub resources: RenderResources,
    /// The render passes and their order, recorded by the render stage.
    pub render_graph: RenderGraph,
    /// Optional cross-instance shader and render-pipeline cache on this device.
    pub shared_pipelines: Option<resource::SharedPipelines>,
}

#[deny(missing_docs)]
impl Renderer {
    /// Requests a window-compatible adapter/device and configures the presentation surface.
    /// Returns host-handle, surface-creation, adapter-selection or device-request errors.
    /// The presentation surface retains a cloned window-handle owner until it is dropped.
    pub async fn initialize<MW>(
        window: &MW,
        wgpu_settings: WgpuSettings,
        settings: RendererSettings,
    ) -> Result<Self, RenderError>
    where
        MW: MapWindow + HeadedMapWindow,
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu_settings.backends.unwrap_or(wgpu::Backends::all()),
            display: window.owned_display_handle(),
            flags: Default::default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface: wgpu::Surface<'static> = instance.create_surface(window.handle().clone())?;

        let (adapter, device, queue) = Self::request_device(
            &instance,
            &wgpu_settings,
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu_settings.power_preference,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                ..Default::default()
            },
        )
        .await?;

        let settings = settings
            .with_float_depth_if_supported(device.features())
            .with_backend_msaa(adapter.get_info().backend);
        let mut surface = Surface::from_surface(surface, &adapter, window, &settings);

        let initial_size = surface.size();
        if let Head::Headed(window) = surface.head_mut() {
            window.resize_and_configure(initial_size.width(), initial_size.height(), &device);
        }
        surface.reconfigure(&device);

        Ok(Self {
            instance,
            device: share_gpu(device),
            queue: upload_queue::UploadQueue::new(queue),
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
            shared_pipelines: None,
        })
    }

    /// Attaches a cross-instance [`resource::SharedPipelines`] cache so pipelines compiled during
    /// this renderer's schedule runs are shared across maps on the same device.
    pub fn with_shared_pipelines(mut self, pipelines: resource::SharedPipelines) -> Self {
        self.shared_pipelines = Some(pipelines);
        self
    }

    /// Requests an adapter/device and allocates an offscreen target at the supplied window size.
    /// The window is not retained. Adapter/device failures are returned; unsupported texture
    /// dimensions, formats or usages are reported by wgpu validation.
    pub async fn initialize_headless<MW>(
        window: &MW,
        wgpu_settings: WgpuSettings,
        settings: RendererSettings,
    ) -> Result<Self, RenderError>
    where
        MW: MapWindow,
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu_settings.backends.unwrap_or(wgpu::Backends::all()),
            flags: Default::default(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let (adapter, device, queue) = Self::request_device(
            &instance,
            &wgpu_settings,
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu_settings.power_preference,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            },
        )
        .await?;

        let settings = settings
            .with_float_depth_if_supported(device.features())
            .with_backend_msaa(adapter.get_info().backend);
        let surface = Surface::from_image(&device, &adapter, window, &settings);

        Ok(Self {
            instance,
            device: share_gpu(device),
            queue: upload_queue::UploadQueue::new(queue),
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
            shared_pipelines: None,
        })
    }

    /// Records physical-pixel dimensions; the prepare stage applies the resize to GPU attachments.
    pub fn resize_surface(&mut self, size: PhysicalSize) {
        self.resources.surface.resize(size)
    }

    /// Requests a device
    pub(crate) async fn request_device(
        instance: &wgpu::Instance,
        settings: &WgpuSettings,
        request_adapter_options: &wgpu::RequestAdapterOptions<'_, '_>,
    ) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), RenderError> {
        let adapter = instance.request_adapter(request_adapter_options).await?;

        let trace = if settings.record_trace {
            wgpu::Trace::Directory("wgpu_trace".into())
        } else {
            wgpu::Trace::Off
        };

        // Enable only the optional GPU features the renderer actually consumes rather than every
        // non-experimental capability advertised by the adapter (such as `shader-f16`, clip
        // distances, or texture compression extensions on mobile browsers).
        const ENGINE_FEATURES: wgpu::Features = wgpu::Features::DEPTH32FLOAT_STENCIL8
            .union(wgpu::Features::INDIRECT_FIRST_INSTANCE)
            .union(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
            .union(wgpu::Features::POLYGON_MODE_LINE)
            .union(wgpu::Features::TIMESTAMP_QUERY)
            .union(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);

        let mut features = adapter.features() & ENGINE_FEATURES;
        let mut limits = settings.limits.clone();

        // Enforce the disabled features
        if let Some(disabled_features) = settings.disabled_features {
            features.remove(disabled_features);
        }
        // NOTE: |= is used here to ensure that any explicitly-enabled features are respected.
        features |= settings.features;

        // Enforce the limit constraints
        if let Some(constrained_limits) = settings.constrained_limits.as_ref() {
            limits = limits.or_worse_values_from(constrained_limits);
        }
        // Default limits a device falls short of, such as the visionOS simulator's GPU with
        // 15 inter-stage variables, are lowered to what it offers, so the map still starts and
        // only a pipeline needing more fails, naming the limit. Limits a host asked for are
        // kept, and a device that cannot meet them is an error.
        if settings.limits == wgpu::Limits::default() {
            limits = limits.or_worse_values_from(&adapter.limits());
        }

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: settings.device_label.as_ref().map(|a| a.as_ref()),
                required_features: features,
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::default(),
                trace,
                ..Default::default()
            })
            .await?;
        Ok((adapter, device, queue))
    }

    /// Borrows the backend instance that owns this renderer's adapter and surfaces.
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }
    /// Borrows the device for allocations, capability inspection and command encoding.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    /// Borrows the queue used by the renderer for uploads and submissions.
    pub fn queue(&self) -> &upload_queue::UploadQueue {
        &self.queue
    }
    /// Borrows frame attachments and presentation state without acquiring a new frame.
    pub fn state(&self) -> &RenderResources {
        &self.resources
    }
    /// Borrows the presentation or offscreen storage, including any pending resize.
    pub fn surface(&self) -> &Surface {
        &self.resources.surface
    }
}

mod render_plugin;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
pub use placement_3d::{AltitudeReference, Orientation3d, Placement3d, ViewSlotMask};
pub use render_plugin::{draw_graph, MaskPipeline, RenderPlugin};
pub(crate) mod tile_memory;

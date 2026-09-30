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
        graph::{EmptyNode, RenderGraph},
        main_pass::{MainPassDriverNode, MainPassNode},
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
mod systems;

mod graph_runner;
mod main_pass;
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
pub mod frame_input;
pub mod projection;
pub mod render_commands;
pub mod render_phase;
pub mod settings;
pub mod tile_mesh;
pub mod tile_view_pattern;
pub mod view_state;
pub mod xr;

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
    /// Extract data from the world.
    Extract,

    /// Prepare render resources from the extracted data for the GPU.
    /// For example during this phase textures are created, buffers are allocated and written.
    Prepare,

    /// Queues [PhaseItems](render_phase::PhaseItem) that depend on
    /// [`Prepare`](RenderStageLabel::Prepare) data and queue up draw calls to run during the
    /// [`Render`](RenderStageLabel::Render) stage.
    /// For example data is uploaded to the GPU in this stage.
    Queue,

    /// Sort the [`RenderPhases`](crate::render::render_phase::RenderPhase) here.
    PhaseSort,

    /// Actual rendering happens here.
    /// In most cases, only the render backend should insert resources here.
    Render,

    /// Cleanup render resources here.
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
    /// Queue for buffer uploads and encoded frame submissions.
    pub queue: wgpu::Queue,
    /// Selected adapter, used to inspect backend capabilities and format support.
    pub adapter: wgpu::Adapter,

    /// Requested device configuration; actual enabled capabilities are available from the device.
    pub wgpu_settings: WgpuSettings,
    /// Render settings adjusted for backend depth and MSAA support during initialization.
    /// Changing these fields does not automatically rebuild existing pipelines.
    pub settings: RendererSettings,

    /// Presentation state and attachments shared by graph nodes.
    pub resources: RenderResources,
    /// Pass dependencies and subgraphs executed by the render stage.
    pub render_graph: RenderGraph,
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
        let surface = Surface::from_surface(surface, &adapter, window, &settings);

        match surface.head() {
            Head::Headed(window) => window.configure(&device),
            Head::Headless(_) => {}
        }

        Ok(Self {
            instance,
            device: share_gpu(device),
            queue,
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
        })
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
            queue,
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
        })
    }

    /// Records physical-pixel dimensions; the prepare stage applies the resize to GPU attachments.
    pub fn resize_surface(&mut self, size: PhysicalSize) {
        self.resources.surface.resize(size)
    }

    /// Requests a device
    async fn request_device(
        instance: &wgpu::Instance,
        settings: &WgpuSettings,
        request_adapter_options: &wgpu::RequestAdapterOptions<'_, '_>,
    ) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), RenderError> {
        let adapter = instance.request_adapter(request_adapter_options).await?;

        let adapter_info = adapter.get_info();

        let trace = if settings.record_trace {
            wgpu::Trace::Directory("wgpu_trace".into())
        } else {
            wgpu::Trace::Off
        };

        let mut features = adapter.features() - wgpu::Features::all_experimental_mask();
        if adapter_info.device_type == wgpu::DeviceType::DiscreteGpu {
            // `MAPPABLE_PRIMARY_BUFFERS` can have a significant, negative performance impact for
            // discrete GPUs due to having to transfer data across the PCI-E bus and so it
            // should not be automatically enabled in this case. It is however beneficial for
            // integrated GPUs.
            features.remove(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
        }
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
    pub fn queue(&self) -> &wgpu::Queue {
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
pub use render_plugin::{draw_graph, main_graph, MaskPipeline, RenderPlugin};
pub(crate) mod tile_memory;

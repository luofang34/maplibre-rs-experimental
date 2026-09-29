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
        resource::{Head, Surface, Texture, TextureView},
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

// Rendering internals
mod graph_runner;
mod main_pass;
pub mod shaders;
mod translucent_pass; // TODO: Make private

// Public API
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

pub struct RenderResources {
    pub surface: Surface,
    pub render_target: Eventually<TextureView>,
    pub depth_texture: Eventually<Texture>,
    pub multisampling_texture: Eventually<Option<Texture>>,
    /// A host's `Depth32Float` texture the frame's depth is copied into after everything is
    /// drawn, for a compositor that reprojects the frame.
    pub eye_depth_target: Option<wgpu::TextureView>,
}

impl RenderResources {
    pub fn new(surface: Surface) -> Self {
        Self {
            render_target: Default::default(),
            depth_texture: Default::default(),
            multisampling_texture: Default::default(),
            eye_depth_target: None,
            surface,
        }
    }

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

    pub fn surface(&self) -> &Surface {
        &self.surface
    }
}

pub struct Renderer {
    pub instance: wgpu::Instance,
    pub device: Arc<wgpu::Device>, // TODO: Arc is needed for headless rendering. Is there a simpler solution?
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,

    pub wgpu_settings: WgpuSettings,
    pub settings: RendererSettings,

    pub resources: RenderResources,
    pub render_graph: RenderGraph,
}

impl Renderer {
    /// Initializes the renderer by retrieving and preparing the GPU instance, device and queue
    /// for the specified backend.
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

        let surface: wgpu::Surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window.handle())?)?
        };

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
            device: Arc::new(device),
            queue,
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
        })
    }

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
            device: Arc::new(device),
            queue,
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
        })
    }

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

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn state(&self) -> &RenderResources {
        &self.resources
    }
    pub fn surface(&self) -> &Surface {
        &self.resources.surface
    }
}

mod render_plugin;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
pub use render_plugin::{draw_graph, main_graph, MaskPipeline, RenderPlugin};
pub(crate) mod tile_memory;

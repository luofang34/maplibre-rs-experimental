//! Cross-instance cache of compiled [`wgpu::ShaderModule`], [`wgpu::BindGroupLayout`], and
//! [`wgpu::RenderPipeline`] objects on a shared [`wgpu::Device`].

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use super::{
    pipeline::RenderPipelineDescriptor,
    shader::{FragmentState, VertexState},
};

thread_local! {
    static ACTIVE_PIPELINES: RefCell<Option<SharedPipelines>> = const { RefCell::new(None) };
}

/// Cloneable cache of compiled WGSL shader modules, canonical bind-group layouts, and render
/// pipelines shared across [`Renderer`](crate::render::Renderer) instances on the same device.
#[derive(Clone, Debug, Default)]
pub struct SharedPipelines(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    shader_compiles: AtomicUsize,
    shader_hits: AtomicUsize,
    pipeline_compiles: AtomicUsize,
    pipeline_hits: AtomicUsize,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            shader_compiles: AtomicUsize::new(0),
            shader_hits: AtomicUsize::new(0),
            pipeline_compiles: AtomicUsize::new(0),
            pipeline_hits: AtomicUsize::new(0),
        }
    }
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPipelines")
            .field(
                "shader_compiles",
                &self.shader_compiles.load(Ordering::Relaxed),
            )
            .field(
                "pipeline_compiles",
                &self.pipeline_compiles.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    shaders: HashMap<(wgpu::Device, &'static str), wgpu::ShaderModule>,
    layouts: HashMap<(wgpu::Device, Vec<wgpu::BindGroupLayoutEntry>), wgpu::BindGroupLayout>,
    pipelines: HashMap<PipelineCacheKey, wgpu::RenderPipeline>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DepthStencilKey {
    format: wgpu::TextureFormat,
    depth_write_enabled: Option<bool>,
    depth_compare: Option<wgpu::CompareFunction>,
    stencil: wgpu::StencilState,
    bias_constant: i32,
    bias_slope_scale_bits: u32,
    bias_clamp_bits: u32,
}

impl From<&wgpu::DepthStencilState> for DepthStencilKey {
    fn from(state: &wgpu::DepthStencilState) -> Self {
        Self {
            format: state.format,
            depth_write_enabled: state.depth_write_enabled,
            depth_compare: state.depth_compare,
            stencil: state.stencil.clone(),
            bias_constant: state.bias.constant,
            bias_slope_scale_bits: state.bias.slope_scale.to_bits(),
            bias_clamp_bits: state.bias.clamp.to_bits(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PipelineCacheKey {
    device: wgpu::Device,
    label: Option<Cow<'static, str>>,
    prefix_layouts: Vec<wgpu::BindGroupLayout>,
    owned_layouts: Option<Vec<Vec<wgpu::BindGroupLayoutEntry>>>,
    vertex: VertexState,
    fragment: FragmentState,
    primitive: wgpu::PrimitiveState,
    depth_stencil: Option<DepthStencilKey>,
    multisample: wgpu::MultisampleState,
}

/// RAII guard restoring the previous thread-local [`SharedPipelines`] scope on drop.
pub(crate) struct PipelineCacheScope {
    previous: Option<SharedPipelines>,
}

impl Drop for PipelineCacheScope {
    fn drop(&mut self) {
        let prev = self.previous.take();
        ACTIVE_PIPELINES.with(|slot| {
            *slot.borrow_mut() = prev;
        });
    }
}

impl SharedPipelines {
    /// Creates an empty pipeline cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of WGSL shader modules compiled on the device through this cache.
    pub fn shader_compile_count(&self) -> usize {
        self.0.shader_compiles.load(Ordering::Relaxed)
    }

    /// Number of cached WGSL shader module hits.
    pub fn shader_hit_count(&self) -> usize {
        self.0.shader_hits.load(Ordering::Relaxed)
    }

    /// Number of [`wgpu::RenderPipeline`] objects compiled on the device through this cache.
    pub fn pipeline_compile_count(&self) -> usize {
        self.0.pipeline_compiles.load(Ordering::Relaxed)
    }

    /// Number of [`wgpu::RenderPipeline`] cache hits across renderers sharing this cache.
    pub fn pipeline_hit_count(&self) -> usize {
        self.0.pipeline_hits.load(Ordering::Relaxed)
    }

    pub(crate) fn enter(cache: Option<Self>) -> PipelineCacheScope {
        let previous =
            ACTIVE_PIPELINES.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), cache));
        PipelineCacheScope { previous }
    }

    pub(crate) fn active() -> Option<Self> {
        ACTIVE_PIPELINES.with(|slot| slot.borrow().clone())
    }

    /// Returns a canonical [`wgpu::BindGroupLayout`] for `(device, entries)`, creating it once.
    pub fn bind_group_layout(
        &self,
        device: &wgpu::Device,
        label: Option<&str>,
        entries: &[wgpu::BindGroupLayoutEntry],
    ) -> wgpu::BindGroupLayout {
        let key = (device.clone(), entries.to_vec());
        let mut state = crate::platform::lock_unpoisoned(&self.0.state);
        if let Some(existing) = state.layouts.get(&key) {
            return existing.clone();
        }
        let created =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label, entries });
        state.layouts.insert(key, created.clone());
        created
    }

    /// Returns a compiled [`wgpu::ShaderModule`] for `(device, source)`, compiling WGSL once.
    pub fn shader_module(
        &self,
        device: &wgpu::Device,
        label: Option<&str>,
        source: &'static str,
    ) -> wgpu::ShaderModule {
        let key = (device.clone(), source);
        let mut state = crate::platform::lock_unpoisoned(&self.0.state);
        if let Some(existing) = state.shaders.get(&key) {
            self.0.shader_hits.fetch_add(1, Ordering::Relaxed);
            return existing.clone();
        }
        let created = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.0.shader_compiles.fetch_add(1, Ordering::Relaxed);
        state.shaders.insert(key, created.clone());
        created
    }

    /// Returns a compiled [`wgpu::RenderPipeline`] matching `(device, descriptor, prefix_layouts)`.
    pub fn render_pipeline(
        &self,
        device: &wgpu::Device,
        descriptor: &RenderPipelineDescriptor,
        prefix_layouts: &[&wgpu::BindGroupLayout],
    ) -> wgpu::RenderPipeline {
        let key = PipelineCacheKey {
            device: device.clone(),
            label: descriptor.label.clone(),
            prefix_layouts: prefix_layouts
                .iter()
                .map(|layout| (*layout).clone())
                .collect(),
            owned_layouts: descriptor.layout.clone(),
            vertex: descriptor.vertex.clone(),
            fragment: descriptor.fragment.clone(),
            primitive: descriptor.primitive,
            depth_stencil: descriptor.depth_stencil.as_ref().map(DepthStencilKey::from),
            multisample: descriptor.multisample,
        };
        {
            let state = crate::platform::lock_unpoisoned(&self.0.state);
            if let Some(existing) = state.pipelines.get(&key) {
                self.0.pipeline_hits.fetch_add(1, Ordering::Relaxed);
                return existing.clone();
            }
        }
        let owned_layouts: Vec<wgpu::BindGroupLayout> = descriptor
            .layout
            .as_ref()
            .map(|groups| {
                groups
                    .iter()
                    .map(|entries| self.bind_group_layout(device, None, entries))
                    .collect()
            })
            .unwrap_or_default();
        let all_layouts: Vec<Option<&wgpu::BindGroupLayout>> = prefix_layouts
            .iter()
            .copied()
            .chain(owned_layouts.iter())
            .map(Some)
            .collect();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &all_layouts,
            ..Default::default()
        });
        let vertex_module = self.shader_module(device, None, descriptor.vertex.source);
        let fragment_module = self.shader_module(device, None, descriptor.fragment.source);
        let vertex_buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = descriptor
            .vertex
            .buffers
            .iter()
            .map(|layout| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: layout.array_stride,
                    step_mode: layout.step_mode,
                    attributes: layout.attributes.as_slice(),
                })
            })
            .collect();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: descriptor.label.as_deref(),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_module,
                entry_point: Some(descriptor.vertex.entry_point),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_module,
                entry_point: Some(descriptor.fragment.entry_point),
                compilation_options: Default::default(),
                targets: descriptor.fragment.targets.as_slice(),
            }),
            primitive: descriptor.primitive,
            depth_stencil: descriptor.depth_stencil.clone(),
            multisample: descriptor.multisample,
            multiview_mask: None,
            cache: None,
        });
        self.0.pipeline_compiles.fetch_add(1, Ordering::Relaxed);
        let mut state = crate::platform::lock_unpoisoned(&self.0.state);
        state.pipelines.insert(key, pipeline.clone());
        pipeline
    }
}

/// Creates or retrieves a canonical [`wgpu::BindGroupLayout`] when a [`SharedPipelines`] scope is
/// active on this thread, falling back to `device.create_bind_group_layout`.
pub(crate) fn create_bind_group_layout_cached(
    device: &wgpu::Device,
    label: Option<&str>,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    if let Some(cache) = SharedPipelines::active() {
        cache.bind_group_layout(device, label, entries)
    } else {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label, entries })
    }
}

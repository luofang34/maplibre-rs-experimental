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

use super::shader::{FragmentState, VertexState};

thread_local! {
    static ACTIVE_PIPELINES: RefCell<Option<Arc<Inner>>> = const { RefCell::new(None) };
}

/// Cloneable cache of compiled WGSL shader modules, canonical bind-group layouts, and render
/// pipelines shared across [`Renderer`](crate::render::Renderer) instances on the same device.
#[derive(Clone, Debug, Default)]
pub struct SharedPipelines(Arc<Inner>);

pub(super) struct Inner {
    state: Mutex<State>,
    evict_shaders_after_frame: bool,
    shader_compiles: AtomicUsize,
    shader_hits: AtomicUsize,
    pipeline_compiles: AtomicUsize,
    pipeline_hits: AtomicUsize,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            evict_shaders_after_frame: false,
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
pub(super) struct DepthStencilKey {
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
pub(super) struct PipelineCacheKey {
    pub(super) device: wgpu::Device,
    pub(super) label: Option<Cow<'static, str>>,
    pub(super) prefix_layouts: Vec<wgpu::BindGroupLayout>,
    pub(super) owned_layouts: Option<Vec<Vec<wgpu::BindGroupLayoutEntry>>>,
    pub(super) vertex: VertexState,
    pub(super) fragment: FragmentState,
    pub(super) primitive: wgpu::PrimitiveState,
    pub(super) depth_stencil: Option<DepthStencilKey>,
    pub(super) multisample: wgpu::MultisampleState,
}

/// RAII guard restoring the previous thread-local [`SharedPipelines`] scope on drop.
pub(crate) struct PipelineCacheScope {
    previous: Option<Arc<Inner>>,
}

impl PipelineCacheScope {
    pub(crate) fn enter(cache: Option<SharedPipelines>) -> Self {
        let next = cache.map(|pipelines| pipelines.0);
        let previous =
            ACTIVE_PIPELINES.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), next));
        Self { previous }
    }
}

impl Drop for PipelineCacheScope {
    fn drop(&mut self) {
        let prev = self.previous.take();
        ACTIVE_PIPELINES.with(|slot| {
            *slot.borrow_mut() = prev;
        });
    }
}

/// Returns the [`SharedPipelines`] active in the current thread-local scope, if any.
pub(crate) fn active_pipelines() -> Option<SharedPipelines> {
    ACTIVE_PIPELINES.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|inner| SharedPipelines(Arc::clone(inner)))
    })
}

impl SharedPipelines {
    /// Creates an empty pipeline cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a per-renderer pipeline cache whose compiled [`wgpu::ShaderModule`] entries are
    /// released at the end of each frame after pipelines are built, freeing Naga/`wgpu_hal` memory.
    pub(crate) fn new_transient() -> Self {
        Self(super::share_gpu(Inner {
            evict_shaders_after_frame: true,
            ..Default::default()
        }))
    }

    /// Clears transient shader modules when [`Self::new_transient`] is active so Naga AST / IR
    /// allocations are not pinned across the lifetime of a single renderer.
    pub(crate) fn clear_transient_shaders(&self) {
        if !self.0.evict_shaders_after_frame {
            return;
        }
        let mut state = crate::util::sync::lock_unpoisoned(&self.0.state);
        if !state.shaders.is_empty() {
            state.shaders.clear();
            state.shaders.shrink_to_fit();
        }
        if !state.layouts.is_empty() {
            state.layouts.clear();
            state.layouts.shrink_to_fit();
        }
        if !state.pipelines.is_empty() {
            state.pipelines.clear();
            state.pipelines.shrink_to_fit();
        }
    }

    /// Evicts all cached shader modules, bind-group layouts, and pipelines belonging to `device`.
    pub fn evict_device(&self, device: &wgpu::Device) {
        let mut state = crate::util::sync::lock_unpoisoned(&self.0.state);
        state.shaders.retain(|(dev, _), _| dev != device);
        state.layouts.retain(|(dev, _), _| dev != device);
        state.pipelines.retain(|key, _| &key.device != device);
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

    /// Returns a canonical [`wgpu::BindGroupLayout`] for `(device, entries)`, creating it once.
    pub fn bind_group_layout(
        &self,
        device: &wgpu::Device,
        label: Option<&str>,
        entries: &[wgpu::BindGroupLayoutEntry],
    ) -> wgpu::BindGroupLayout {
        let key = (device.clone(), entries.to_vec());
        let mut state = crate::util::sync::lock_unpoisoned(&self.0.state);
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
        let mut state = crate::util::sync::lock_unpoisoned(&self.0.state);
        if let Some(existing) = state.shaders.get(&key) {
            self.0.shader_hits.fetch_add(1, Ordering::Relaxed);
            return existing.clone();
        }
        let created = super::shader::create_shader_module_uncached(device, label, source);
        self.0.shader_compiles.fetch_add(1, Ordering::Relaxed);
        state.shaders.insert(key, created.clone());
        created
    }

    /// Looks up a compiled [`wgpu::RenderPipeline`] under `key`, building and caching it via
    /// `compile` on miss.
    pub(super) fn get_or_compile_pipeline(
        &self,
        key: PipelineCacheKey,
        compile: impl FnOnce(&Self) -> wgpu::RenderPipeline,
    ) -> wgpu::RenderPipeline {
        {
            let state = crate::util::sync::lock_unpoisoned(&self.0.state);
            if let Some(existing) = state.pipelines.get(&key) {
                self.0.pipeline_hits.fetch_add(1, Ordering::Relaxed);
                return existing.clone();
            }
        }
        let pipeline = compile(self);
        self.0.pipeline_compiles.fetch_add(1, Ordering::Relaxed);
        let mut state = crate::util::sync::lock_unpoisoned(&self.0.state);
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
    if let Some(cache) = active_pipelines() {
        cache.bind_group_layout(device, label, entries)
    } else {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label, entries })
    }
}

/// Returns a cached [`wgpu::ShaderModule`] from the active [`SharedPipelines`] scope when entered,
/// or creates one directly otherwise.
pub(crate) fn create_shader_module_from_wgsl(
    device: &wgpu::Device,
    label: Option<&str>,
    source: &'static str,
) -> wgpu::ShaderModule {
    if let Some(cache) = active_pipelines() {
        return cache.shader_module(device, label, source);
    }
    super::shader::create_shader_module_uncached(device, label, source)
}

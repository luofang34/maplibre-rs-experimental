//! Compilation of owned shader and fixed-function descriptions into GPU render pipelines.

use std::borrow::Cow;

pub use super::shader::RenderPipelineDescriptor;
use crate::render::resource::shader::VertexState;

/// Consumes a pipeline configuration into a descriptor without allocating GPU resources.
pub trait RenderPipeline {
    /// Produces shader, binding and fixed-function state for later device initialization.
    fn describe_render_pipeline(self) -> RenderPipelineDescriptor;
}

/// How [`FeaturePipelinePair::uniform`] sources its uniform feature attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniformFeatureBinding {
    /// Flips `vertex.buffers[slot].step_mode` to [`wgpu::VertexStepMode::Instance`].
    InstanceSlot(usize),
    /// Truncates `vertex.buffers` to `feature_slot` and appends `attribute` onto
    /// `vertex.buffers[layer_slot]` (`VertexStepMode::Instance`), eliminating the feature buffer
    /// binding slot completely for uniform draws.
    LayerMetadata {
        /// Existing instance-stepped layer metadata buffer slot.
        layer_slot: usize,
        /// Per-vertex feature metadata buffer slot to truncate from the uniform pipeline.
        feature_slot: usize,
        /// Uniform color attribute read from the layer metadata buffer.
        attribute: wgpu::VertexAttribute,
    },
}

impl UniformFeatureBinding {
    fn apply(self, vertex: &mut VertexState) {
        match self {
            Self::InstanceSlot(slot) => {
                if let Some(buf) = vertex.buffers.get_mut(slot) {
                    buf.step_mode = wgpu::VertexStepMode::Instance;
                }
            }
            Self::LayerMetadata {
                layer_slot,
                feature_slot,
                attribute,
            } => {
                vertex.buffers.truncate(feature_slot);
                if let Some(buf) = vertex.buffers.get_mut(layer_slot) {
                    buf.attributes.push(attribute);
                }
            }
        }
    }
}

/// Pair of render pipelines sharing one compiled WGSL shader pair and pipeline layout:
/// `per_vertex` steps `FillShaderFeatureMetadata` per vertex, while `uniform` sources uniform
/// feature paint from either an instance-stepped record or [`ShaderLayerMetadata`](crate::render::shaders::ShaderLayerMetadata).
pub struct FeaturePipelinePair {
    /// Pipeline stepping the feature-metadata buffer with [`wgpu::VertexStepMode::Vertex`].
    pub per_vertex: wgpu::RenderPipeline,
    /// Pipeline sourcing uniform feature paint per instance without per-vertex feature attributes.
    pub uniform: wgpu::RenderPipeline,
}

impl FeaturePipelinePair {
    /// Selects the `uniform` (`VertexStepMode::Instance`) or `per_vertex` variant.
    #[inline]
    pub fn select(&self, uniform: bool) -> &wgpu::RenderPipeline {
        if uniform {
            &self.uniform
        } else {
            &self.per_vertex
        }
    }
}

impl RenderPipelineDescriptor {
    /// Compiles the shaders and creates a pipeline with only the descriptor-owned bind groups.
    /// Invalid WGSL, incompatible bindings or unsupported states use wgpu's validation mechanism.
    pub fn initialize(&self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        self.initialize_with_prefix_layouts(device, &[])
    }

    /// Creates a pipeline with shared bind-group layouts before descriptor-owned layouts.
    /// Shader group indices must account for the prefix length. Prefix layouts and shader
    /// interfaces must be compatible with `device`; validation errors use wgpu's error scopes
    /// or uncaptured-error handler. When a [`SharedPipelines`](super::SharedPipelines) scope is
    /// active on the calling thread, compiled shader modules and pipelines are reused.
    pub fn initialize_with_prefix_layouts(
        &self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
    ) -> wgpu::RenderPipeline {
        if let Some(cache) = super::pipeline_cache::active_pipelines() {
            return self.render_pipeline_cached(&cache, device, prefix_layouts);
        }
        let pipeline_layout = self.create_pipeline_layout(device, prefix_layouts);
        let (vertex_module, fragment_module) = self.compile_shader_modules(device);
        self.create_render_pipeline(
            device,
            &pipeline_layout,
            (&vertex_module, &fragment_module),
            self.label.as_deref(),
        )
    }

    /// Compiles `(per_vertex, uniform)` render pipelines sharing one [`wgpu::PipelineLayout`] and
    /// compiled [`wgpu::ShaderModule`] pair, flipping `vertex.buffers[uniform_slot].step_mode` to
    /// [`wgpu::VertexStepMode::Instance`] for the `uniform` variant.
    pub fn initialize_pair_with_prefix_layouts(
        self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
        uniform_slot: usize,
    ) -> FeaturePipelinePair {
        self.initialize_pair_with_binding(
            device,
            prefix_layouts,
            UniformFeatureBinding::InstanceSlot(uniform_slot),
        )
    }

    /// Compiles `(per_vertex, uniform)` render pipelines sharing one [`wgpu::PipelineLayout`] and
    /// compiled [`wgpu::ShaderModule`] pair, transforming `vertex.buffers` via `binding` for the
    /// `uniform` variant.
    pub fn initialize_pair_with_binding(
        mut self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
        binding: UniformFeatureBinding,
    ) -> FeaturePipelinePair {
        if let Some(cache) = super::pipeline_cache::active_pipelines() {
            let per_vertex = self.render_pipeline_cached(&cache, device, prefix_layouts);
            binding.apply(&mut self.vertex);
            if let Some(label) = self.label.take() {
                self.label = Some(Cow::Owned(format!("{label}_uniform")));
            }
            let uniform = self.render_pipeline_cached(&cache, device, prefix_layouts);
            return FeaturePipelinePair {
                per_vertex,
                uniform,
            };
        }
        let pipeline_layout = self.create_pipeline_layout(device, prefix_layouts);
        let (vertex_module, fragment_module) = self.compile_shader_modules(device);
        let per_vertex = self.create_render_pipeline(
            device,
            &pipeline_layout,
            (&vertex_module, &fragment_module),
            self.label.as_deref(),
        );
        binding.apply(&mut self.vertex);
        let uniform_label = self.label.as_deref().map(|l| format!("{l}_uniform"));
        let uniform = self.create_render_pipeline(
            device,
            &pipeline_layout,
            (&vertex_module, &fragment_module),
            uniform_label.as_deref(),
        );
        FeaturePipelinePair {
            per_vertex,
            uniform,
        }
    }

    fn create_pipeline_layout(
        &self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
    ) -> wgpu::PipelineLayout {
        let bind_group_layouts = self
            .layout
            .as_ref()
            .map(|layout| {
                layout
                    .iter()
                    .map(|entries| {
                        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                            label: None,
                            entries: entries.as_ref(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let all_bind_group_layouts = prefix_layouts
            .iter()
            .copied()
            .chain(bind_group_layouts.iter())
            .map(Some)
            .collect::<Vec<_>>();
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &all_bind_group_layouts,
            ..Default::default()
        })
    }

    fn compile_shader_modules(
        &self,
        device: &wgpu::Device,
    ) -> (wgpu::ShaderModule, wgpu::ShaderModule) {
        let vertex_module =
            super::pipeline_cache::create_shader_module_from_wgsl(device, None, self.vertex.source);
        let fragment_module = super::pipeline_cache::create_shader_module_from_wgsl(
            device,
            None,
            self.fragment.source,
        );
        (vertex_module, fragment_module)
    }

    fn create_render_pipeline(
        &self,
        device: &wgpu::Device,
        pipeline_layout: &wgpu::PipelineLayout,
        (vertex_module, fragment_module): (&wgpu::ShaderModule, &wgpu::ShaderModule),
        label: Option<&str>,
    ) -> wgpu::RenderPipeline {
        let buffers = self
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
            .collect::<Vec<_>>();
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label,
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: vertex_module,
                entry_point: Some(self.vertex.entry_point),
                compilation_options: Default::default(),
                buffers: buffers.as_slice(),
            },
            fragment: Some(wgpu::FragmentState {
                module: fragment_module,
                entry_point: Some(self.fragment.entry_point),
                compilation_options: Default::default(),
                targets: self.fragment.targets.as_slice(),
            }),
            primitive: self.primitive,
            depth_stencil: self.depth_stencil.clone(),
            multisample: self.multisample,
            multiview_mask: None,
            cache: None,
        })
    }

    fn pipeline_cache_key(
        &self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
    ) -> super::pipeline_cache::PipelineCacheKey {
        super::pipeline_cache::PipelineCacheKey {
            device: device.clone(),
            label: self.label.clone(),
            prefix_layouts: prefix_layouts
                .iter()
                .map(|layout| (*layout).clone())
                .collect(),
            owned_layouts: self.layout.clone(),
            vertex: self.vertex.clone(),
            fragment: self.fragment.clone(),
            primitive: self.primitive,
            depth_stencil: self
                .depth_stencil
                .as_ref()
                .map(super::pipeline_cache::DepthStencilKey::from),
            multisample: self.multisample,
        }
    }

    fn render_pipeline_cached(
        &self,
        cache: &super::pipeline_cache::SharedPipelines,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
    ) -> wgpu::RenderPipeline {
        let key = self.pipeline_cache_key(device, prefix_layouts);
        cache.get_or_compile_pipeline(key, |pipelines| {
            let owned_layouts: Vec<wgpu::BindGroupLayout> = self
                .layout
                .as_ref()
                .map(|groups| {
                    groups
                        .iter()
                        .map(|entries| pipelines.bind_group_layout(device, None, entries))
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
            let vertex_module = pipelines.shader_module(device, None, self.vertex.source);
            let fragment_module = pipelines.shader_module(device, None, self.fragment.source);
            self.create_render_pipeline(
                device,
                &pipeline_layout,
                (&vertex_module, &fragment_module),
                self.label.as_deref(),
            )
        })
    }
}

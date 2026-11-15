//! Compilation of owned shader and fixed-function descriptions into GPU render pipelines.

use std::borrow::Cow;

use crate::render::resource::shader::{FragmentState, VertexState};

/// Consumes a pipeline configuration into a descriptor without allocating GPU resources.
pub trait RenderPipeline {
    /// Produces shader, binding and fixed-function state for later device initialization.
    fn describe_render_pipeline(self) -> RenderPipelineDescriptor;
}

/// Pair of render pipelines sharing one compiled WGSL shader pair and pipeline layout:
/// `per_vertex` steps `FillShaderFeatureMetadata` per vertex, while `uniform` steps a single
/// 16-byte `FillShaderFeatureMetadata` record per instance (`0..1`).
pub struct FeaturePipelinePair {
    /// Pipeline stepping the feature-metadata buffer with [`wgpu::VertexStepMode::Vertex`].
    pub per_vertex: wgpu::RenderPipeline,
    /// Pipeline stepping the feature-metadata buffer with [`wgpu::VertexStepMode::Instance`].
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

/// Owned pipeline state, including WGSL sources compiled at initialization.
pub struct RenderPipelineDescriptor {
    /// Debug label of the pipeline. This will show up in graphics debuggers for easy identification.
    pub label: Option<Cow<'static, str>>,
    /// Descriptor-owned bind groups in shader group-index order, after any prefix layouts.
    /// `None` adds no groups; it does not request wgpu's automatic layout inference.
    pub layout: Option<Vec<Vec<wgpu::BindGroupLayoutEntry>>>,
    /// Vertex shader source, entry point and ordered input buffer layouts.
    pub vertex: VertexState,
    /// The properties of the pipeline at the primitive assembly and rasterization level.
    pub primitive: wgpu::PrimitiveState,
    /// The effect of draw calls on the depth and stencil aspects of the output target, if any.
    pub depth_stencil: Option<wgpu::DepthStencilState>,
    /// The multi-sampling properties of the pipeline.
    pub multisample: wgpu::MultisampleState,
    /// Fragment shader source, entry point and color attachment formats and blend states.
    pub fragment: FragmentState,
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
        if let Some(cache) = super::pipeline_cache::SharedPipelines::active() {
            return cache.render_pipeline(device, self, prefix_layouts);
        }
        let bind_group_layouts = if let Some(layout) = &self.layout {
            layout
                .iter()
                .map(|entries| {
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: None,
                        entries: entries.as_ref(),
                    })
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        let all_bind_group_layouts = prefix_layouts
            .iter()
            .copied()
            .chain(bind_group_layouts.iter())
            .map(Some)
            .collect::<Vec<_>>();

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &all_bind_group_layouts,
            ..Default::default()
        });

        let vertex_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(self.vertex.source.into()),
        });
        let fragment_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(self.fragment.source.into()),
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: self.label.as_ref().map(|label| label.as_ref()),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader_module,
                entry_point: Some(self.vertex.entry_point),
                compilation_options: Default::default(),
                buffers: self
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
                    .collect::<Vec<_>>()
                    .as_slice(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader_module,
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

    /// Compiles `(per_vertex, uniform)` render pipelines sharing one [`wgpu::PipelineLayout`] and
    /// compiled [`wgpu::ShaderModule`] pair, flipping `vertex.buffers[uniform_slot].step_mode` to
    /// [`wgpu::VertexStepMode::Instance`] for the `uniform` variant.
    pub fn initialize_pair_with_prefix_layouts(
        mut self,
        device: &wgpu::Device,
        prefix_layouts: &[&wgpu::BindGroupLayout],
        uniform_slot: usize,
    ) -> FeaturePipelinePair {
        if let Some(cache) = super::pipeline_cache::SharedPipelines::active() {
            let per_vertex = cache.render_pipeline(device, &self, prefix_layouts);
            if let Some(buf) = self.vertex.buffers.get_mut(uniform_slot) {
                buf.step_mode = wgpu::VertexStepMode::Instance;
            }
            if let Some(label) = self.label.take() {
                self.label = Some(Cow::Owned(format!("{label}_uniform")));
            }
            let uniform = cache.render_pipeline(device, &self, prefix_layouts);
            return FeaturePipelinePair {
                per_vertex,
                uniform,
            };
        }

        let bind_group_layouts = if let Some(layout) = &self.layout {
            layout
                .iter()
                .map(|entries| {
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: None,
                        entries: entries.as_ref(),
                    })
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        let all_bind_group_layouts = prefix_layouts
            .iter()
            .copied()
            .chain(bind_group_layouts.iter())
            .map(Some)
            .collect::<Vec<_>>();

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &all_bind_group_layouts,
            ..Default::default()
        });

        let vertex_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(self.vertex.source.into()),
        });
        let fragment_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(self.fragment.source.into()),
        });

        let build = |desc: &Self, label: Option<&str>| {
            let buffers = desc
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
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vertex_shader_module,
                    entry_point: Some(desc.vertex.entry_point),
                    compilation_options: Default::default(),
                    buffers: buffers.as_slice(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment_shader_module,
                    entry_point: Some(desc.fragment.entry_point),
                    compilation_options: Default::default(),
                    targets: desc.fragment.targets.as_slice(),
                }),
                primitive: desc.primitive,
                depth_stencil: desc.depth_stencil.clone(),
                multisample: desc.multisample,
                multiview_mask: None,
                cache: None,
            })
        };

        let per_vertex = build(&self, self.label.as_deref());
        if let Some(buf) = self.vertex.buffers.get_mut(uniform_slot) {
            buf.step_mode = wgpu::VertexStepMode::Instance;
        }
        let uniform_label = self.label.as_deref().map(|l| format!("{l}_uniform"));
        let uniform = build(&self, uniform_label.as_deref());
        FeaturePipelinePair {
            per_vertex,
            uniform,
        }
    }
}

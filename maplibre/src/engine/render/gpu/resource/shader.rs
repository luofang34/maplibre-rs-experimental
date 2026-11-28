//! Owned vertex layouts and WGSL stage descriptions used to construct render pipelines.

/// Describes how the vertex buffer is interpreted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexBufferLayout {
    /// The stride, in bytes, between elements of this buffer.
    pub array_stride: wgpu::BufferAddress,
    /// How often this vertex buffer is "stepped" forward.
    pub step_mode: wgpu::VertexStepMode,
    /// The list of attributes which comprise a single vertex.
    pub attributes: Vec<wgpu::VertexAttribute>,
}

/// Describes the fragment process in a render pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FragmentState {
    /// WGSL source compiled when the pipeline is initialized.
    pub source: &'static str,
    /// The name of the entry point in the compiled shader. There must be a
    /// function with this name in the shader.
    pub entry_point: &'static str,
    /// The color state of the render targets.
    pub targets: Vec<Option<wgpu::ColorTargetState>>,
}

/// WGSL vertex stage and the ordered buffer layouts bound to its input slots.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexState {
    /// WGSL source compiled when the pipeline is initialized.
    pub source: &'static str,
    /// The name of the entry point in the compiled shader. There must be a
    /// function with this name in the shader.
    pub entry_point: &'static str,
    /// The format of any vertex buffers used with this pipeline.
    pub buffers: Vec<VertexBufferLayout>,
}

/// Owned pipeline state, including WGSL sources compiled at initialization.
pub struct RenderPipelineDescriptor {
    /// Debug label of the pipeline. This will show up in graphics debuggers for easy identification.
    pub label: Option<std::borrow::Cow<'static, str>>,
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

/// Creates a `wgpu::ShaderModule` directly from static WGSL source without cache lookup.
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
pub(super) fn create_shader_module_uncached(
    device: &wgpu::Device,
    label: Option<&str>,
    source: &'static str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label,
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(source)),
    })
}

/// Dummy fallback for render-free `wasm32` worker builds where `wgsl` is not enabled.
#[cfg(all(
    target_arch = "wasm32",
    not(feature = "webgpu"),
    not(feature = "web-webgl")
))]
pub(super) fn create_shader_module_uncached(
    device: &wgpu::Device,
    label: Option<&str>,
    _source: &'static str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label,
        source: wgpu::ShaderSource::Dummy(std::marker::PhantomData),
    })
}

//! Shader module creation helper with a render-free dummy fallback for `web-tile-worker`.

/// Creates a `wgpu::ShaderModule` from static WGSL source.
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
pub(crate) fn create_shader_module_from_wgsl(
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
pub(crate) fn create_shader_module_from_wgsl(
    device: &wgpu::Device,
    label: Option<&str>,
    _source: &'static str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label,
        source: wgpu::ShaderSource::Dummy(std::marker::PhantomData),
    })
}

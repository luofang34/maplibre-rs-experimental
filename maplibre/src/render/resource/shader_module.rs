//! Shader module creation helper with `SharedPipelines` caching and a render-free dummy fallback for `web-tile-worker`.

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

/// Returns a cached [`wgpu::ShaderModule`] from the active [`SharedPipelines`](super::SharedPipelines)
/// scope when entered, or creates one directly otherwise.
pub(crate) fn create_shader_module_from_wgsl(
    device: &wgpu::Device,
    label: Option<&str>,
    source: &'static str,
) -> wgpu::ShaderModule {
    if let Some(cache) = super::pipeline_cache::SharedPipelines::active() {
        return cache.shader_module(device, label, source);
    }
    create_shader_module_uncached(device, label, source)
}

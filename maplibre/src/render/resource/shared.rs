//! Reference-counted sharing of GPU objects whose thread affinity depends on the target.

use std::sync::Arc;

/// The single construction point for shared GPU objects, so the public `Arc` API is identical
/// on every target.
///
/// On wasm32 wgpu handles wrap JavaScript objects and are neither `Send` nor `Sync`; the `Arc`
/// only counts references there, and the compiler rejects any `Send` bound on such a handle.
/// Because the wrapper is generic, `clippy::arc_with_non_send_sync` does not fire at these call
/// sites on any target; the native test below pins the property the lint would otherwise check.
pub(crate) fn share_gpu<T>(value: T) -> Arc<T> {
    Arc::new(value)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use crate::render::resource::BufferedTextureHead;

    #[test]
    fn native_gpu_handles_cross_threads() {
        fn assert_thread_safe<T: Send + Sync>() {}
        assert_thread_safe::<Arc<wgpu::Device>>();
        assert_thread_safe::<Arc<wgpu::BindGroup>>();
        assert_thread_safe::<Arc<BufferedTextureHead>>();
    }
}

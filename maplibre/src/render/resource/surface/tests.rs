#![allow(clippy::expect_used, clippy::panic)]

use super::*;

async fn surface_with_flags(flags: wgpu::TextureFormatFeatureFlags) -> Surface {
    let (_, renderer) = crate::headless::create_headless_renderer(3, 2, None)
        .await
        .expect("headless renderer");
    let mut surface = renderer.resources.surface;
    let Head::Headless(texture) = surface.head_mut() else {
        panic!("offscreen surface");
    };
    Arc::get_mut(texture)
        .expect("unique texture")
        .texture_format_features
        .flags = flags;
    surface
}

#[tokio::test]
async fn non_power_of_two_sample_counts_are_rejected() {
    let surface = surface_with_flags(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4).await;
    assert!(!surface.is_multisampling_supported(Msaa { samples: 3 }));
    assert!(!surface.is_multisampling_supported(Msaa { samples: 0 }));
    assert!(surface.is_multisampling_supported(Msaa { samples: 1 }));
    assert!(surface.is_multisampling_supported(Msaa { samples: 4 }));
}

#[tokio::test]
async fn four_samples_do_not_imply_two_samples_are_supported() {
    let surface = surface_with_flags(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4).await;
    assert!(!surface.is_multisampling_supported(Msaa { samples: 2 }));
}

#[tokio::test]
async fn sixteen_samples_are_supported_when_advertised() {
    let surface = surface_with_flags(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X16).await;
    assert!(surface.is_multisampling_supported(Msaa { samples: 16 }));
    assert!(!surface.is_multisampling_supported(Msaa { samples: 8 }));
}

#[tokio::test]
async fn an_invalid_sample_count_falls_back_to_a_valid_frame() {
    use crate::{
        headless::{create_headless_renderer_with_settings, map::HeadlessMap},
        render::RenderPlugin,
    };
    let settings = RendererSettings {
        msaa: Msaa { samples: 3 },
        ..Default::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(3, 2, None, settings)
        .await
        .expect("headless renderer");
    let device = renderer.device.clone();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let style =
        serde_json::from_str(r#"{"version":8,"sources":{},"layers":[]}"#).expect("empty style");
    let mut map = HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)])
        .expect("headless map");
    map.run_frame().expect("fallback frame");
    assert!(scope.pop().await.is_none(), "invalid GPU resources");
}

#[tokio::test]
async fn resizing_allocates_new_storage_and_retains_existing_capture_handles() {
    let (_, mut renderer) = crate::headless::create_headless_renderer(3, 2, None)
        .await
        .expect("headless renderer");
    let surface = &mut renderer.resources.surface;
    let Head::Headless(original) = surface.head() else {
        panic!("offscreen surface");
    };
    let original = original.clone();
    surface.resize(PhysicalSize::new(65, 3).expect("nonzero size"));
    assert_eq!(surface.size().width(), 65);
    let Head::Headless(pending) = surface.head() else {
        panic!("offscreen surface");
    };
    assert!(Arc::ptr_eq(&original, pending), "resize is deferred");
    surface.reconfigure(&renderer.device);
    let Head::Headless(current) = surface.head() else {
        panic!("offscreen surface");
    };
    assert!(!Arc::ptr_eq(&original, current));
    assert_eq!(
        (current.texture().width(), current.texture().height()),
        (65, 3)
    );
    assert_eq!(current.bytes_per_row(), 512);
    assert_eq!(current.buffer().size(), 512 * 3);
    assert_eq!(
        (original.texture().width(), original.texture().height()),
        (3, 2)
    );
    assert_eq!(original.bytes_per_row(), 256);
    let current = current.clone();
    surface.reconfigure(&renderer.device);
    let Head::Headless(unchanged) = surface.head() else {
        panic!("offscreen surface");
    };
    assert!(
        Arc::ptr_eq(&current, unchanged),
        "unchanged size reuses storage"
    );
}

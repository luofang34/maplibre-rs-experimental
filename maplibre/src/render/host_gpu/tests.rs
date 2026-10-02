use super::*;
use crate::{
    headless::{
        create_headless_renderer, create_headless_renderer_on_host_gpu, map::HeadlessMap,
        HeadlessPlugin,
    },
    render::{settings::Msaa, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 64;

async fn host_gpu() -> Option<HostGpu> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .ok()?;
    Some(HostGpu {
        instance,
        adapter,
        device,
        queue,
    })
}

fn style(color: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8, "sources": {},
        "layers": [{"id": "bg", "type": "background", "paint": {"background-color": color}}]
    }))
    .expect("style")
}

fn map_on(gpu: &HostGpu, format: wgpu::TextureFormat, samples: u32) -> HeadlessMap {
    let settings = RendererSettings {
        texture_format: Some(format),
        msaa: Msaa { samples },
        ..Default::default()
    };
    let (kernel, renderer) =
        create_headless_renderer_on_host_gpu(SIZE, SIZE, gpu.clone(), settings, None)
            .expect("renderer");
    let mut map = HeadlessMap::new(
        style("#ff0000"),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    map.render_source_frames(Default::default(), Vec::new(), 1)
        .expect("frame");
    map
}

const SAMPLE_SHADER: &str = r"
@group(0) @binding(0) var map_texture: texture_2d<f32>;
@group(0) @binding(1) var map_sampler: sampler;
struct Varyings { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Varyings(vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0), corner);
}
@fragment fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return textureSample(map_texture, map_sampler, in.uv);
}
";

/// Draws `texture` into a target of the host's own in a host pass, the way a compositor
/// would, then reads that target back for the test to look at.
fn host_samples(gpu: &HostGpu, texture: &wgpu::Texture) -> Vec<u8> {
    let device = &gpu.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("host compositor"),
        source: wgpu::ShaderSource::Wgsl(SAMPLE_SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("host compositor"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("host target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let view = target.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: None,
            },
        },
        target.size(),
    );
    gpu.queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    let bytes = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped")
        .to_vec();
    buffer.unmap();
    bytes
}

fn all(pixels: &[u8], expected: [u8; 4]) -> bool {
    pixels
        .chunks_exact(4)
        .all(|pixel| pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2))
}

#[tokio::test]
async fn a_host_samples_the_map_in_the_format_it_asked_for() {
    let Some(gpu) = host_gpu().await else {
        return;
    };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba16Float,
    ] {
        let map = map_on(&gpu, format, 1);
        let texture = map.head_texture().expect("offscreen texture");
        assert_eq!(texture.format(), format);
        assert!(texture
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert!(
            all(&host_samples(&gpu, texture), [255, 0, 0, 255]),
            "{format:?}: the host's pass shows the map"
        );
    }
}

#[tokio::test]
async fn a_multisampled_map_resolves_to_a_texture_the_host_samples() {
    let Some(gpu) = host_gpu().await else {
        return;
    };
    let map = map_on(&gpu, wgpu::TextureFormat::Rgba8Unorm, 4);
    let renderer = map.renderer();
    assert_eq!(renderer.settings.msaa.samples, 4);
    assert!(
        matches!(
            &renderer.resources.multisampling_texture,
            crate::render::eventually::Eventually::Initialized(Some(texture))
                if texture.texture.sample_count() == 4
        ),
        "the frame is drawn multisampled"
    );
    let texture = map.head_texture().expect("offscreen texture");
    assert_eq!(texture.sample_count(), 1, "the host samples the resolve");
    assert!(all(&host_samples(&gpu, texture), [255, 0, 0, 255]));
}

#[tokio::test]
async fn a_resize_draws_into_a_new_texture_and_the_host_keeps_the_old_one() {
    let Some(gpu) = host_gpu().await else {
        return;
    };
    let mut map = map_on(&gpu, wgpu::TextureFormat::Rgba8Unorm, 4);
    let before = map.head_texture().expect("offscreen texture").clone();
    map.resize(crate::window::PhysicalSize::new(SIZE * 2, SIZE / 2).expect("size"));
    map.mutate_style(|style| style.set_paint_property("bg", "background-color", "#0000ff".into()))
        .expect("style change");
    map.render_source_frames(Default::default(), Vec::new(), 1)
        .expect("resized frame");
    let after = map.head_texture().expect("offscreen texture").clone();
    assert_ne!(before, after, "the resized frame has its own texture");
    assert_eq!((after.width(), after.height()), (SIZE * 2, SIZE / 2));
    assert!(
        all(&host_samples(&gpu, &after), [0, 0, 255, 255]),
        "the new texture holds the new frame"
    );
    drop(map);
    assert!(
        all(&host_samples(&gpu, &before), [255, 0, 0, 255]),
        "the host's old texture keeps its frame after the map is gone"
    );
}

#[tokio::test]
async fn the_host_device_draws_what_the_map_s_own_device_draws() {
    let Some(gpu) = host_gpu().await else {
        return;
    };
    let hosted = map_on(&gpu, wgpu::TextureFormat::Rgba8Unorm, 1);
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut own = HeadlessMap::new(
        style("#ff0000"),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    own.render_source_frames(Default::default(), Vec::new(), 1)
        .expect("frame");
    let read = |map: &HeadlessMap| {
        crate::headless::map::reference::readback::read_blocking(
            map,
            map.head_texture().expect("texture"),
            wgpu::TextureAspect::All,
        )
        .expect("readback")
    };
    let hosted_pixels = read(&hosted);
    assert!(all(&hosted_pixels, [255, 0, 0, 255]));
    assert_eq!(
        hosted_pixels,
        read(&own),
        "a readback caller sees the same frame on the host's device as on the map's own"
    );
}

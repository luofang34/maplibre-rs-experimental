//! GPU texture, uniform buffer, and bind-group ownership for one evaluated line dash/image entry.

use super::{
    crossfade::{CrossfadeParameters, DashUniforms},
    image::{build_dash_uniforms, ramp_texture, LineImage},
    pixels::{build_dash_texture_pixels, WIDTH},
};
use crate::render::upload_queue::UploadQueue;

pub(super) struct DashSpec<'a> {
    pub(super) from_pattern: &'a [f64],
    pub(super) to_pattern: &'a [f64],
    pub(super) round: bool,
    pub(super) floor_width: f32,
}

pub(super) struct DashEntry {
    pub(super) from_pattern: Vec<f64>,
    pub(super) to_pattern: Vec<f64>,
    /// Whether the dashes end in round caps, which the texture then encodes across the line.
    pub(super) round: bool,
    pub(super) dual_rows: bool,
    pub(super) image: LineImage,
    pub(super) binding: wgpu::BindGroup,
    pub(super) uniform: wgpu::Buffer,
    dash_texture: wgpu::Texture,
    ramp_texture: wgpu::Texture,
    pub(super) from_period: f32,
    pub(super) to_period: f32,
    pub(super) floor_width: f32,
    uniform_data: DashUniforms,
}

impl DashEntry {
    pub(super) fn refresh_uniform(
        &mut self,
        queue: &UploadQueue,
        scale: f32,
        crossfade: CrossfadeParameters,
        floor_width: f32,
    ) {
        self.floor_width = floor_width;
        let next = build_dash_uniforms(
            (self.from_period, self.to_period),
            &self.image,
            self.round,
            scale,
            crossfade,
            floor_width,
            self.dual_rows,
        );
        if self.uniform_data != next {
            self.uniform_data = next;
            queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&next));
        }
    }
}

impl Drop for DashEntry {
    fn drop(&mut self) {
        self.uniform.destroy();
        self.dash_texture.destroy();
        self.ramp_texture.destroy();
    }
}

fn create_dash_texture_view(
    device: &wgpu::Device,
    queue: &UploadQueue,
    pixels: &[u8],
    rows: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("line dash distance"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: rows,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: Some(rows),
        },
        texture.size(),
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (texture, view, sampler)
}

fn create_dash_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    dash: (&wgpu::TextureView, &wgpu::Sampler),
    uniform: &wgpu::Buffer,
    ramp: (&wgpu::TextureView, &wgpu::Sampler),
) -> wgpu::BindGroup {
    use wgpu::BindingResource::{Sampler, TextureView};
    let entries = [
        wgpu::BindGroupEntry {
            binding: 0,
            resource: TextureView(dash.0),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: Sampler(dash.1),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: uniform.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: TextureView(ramp.0),
        },
        wgpu::BindGroupEntry {
            binding: 4,
            resource: Sampler(ramp.1),
        },
    ];
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("line dash"),
        layout,
        entries: &entries,
    })
}

pub(super) fn create_entry(
    device: &wgpu::Device,
    queue: &UploadQueue,
    layout: &wgpu::BindGroupLayout,
    spec: DashSpec<'_>,
    (image, scale, crossfade): (&LineImage, f32, CrossfadeParameters),
) -> DashEntry {
    let (pixels, from_period, to_period, rows, dual_rows) =
        build_dash_texture_pixels(spec.from_pattern, spec.to_pattern, spec.round);
    let (dash_texture, view, sampler) = create_dash_texture_view(device, queue, &pixels, rows);
    let uniform_data = build_dash_uniforms(
        (from_period, to_period),
        image,
        spec.round,
        scale,
        crossfade,
        spec.floor_width,
        dual_rows,
    );
    let uniform = queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("line dash period"),
            contents: bytemuck::bytes_of(&uniform_data),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        },
    );
    let (ramp_texture, ramp_view, ramp_sampler) = ramp_texture(device, queue, image);
    let binding = create_dash_bind_group(
        device,
        layout,
        (&view, &sampler),
        &uniform,
        (&ramp_view, &ramp_sampler),
    );
    DashEntry {
        from_pattern: spec.from_pattern.to_vec(),
        to_pattern: spec.to_pattern.to_vec(),
        round: spec.round,
        dual_rows,
        image: image.clone(),
        binding,
        uniform,
        dash_texture,
        ramp_texture,
        from_period,
        to_period,
        floor_width: spec.floor_width,
        uniform_data,
    }
}

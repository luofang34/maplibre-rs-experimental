//! A small repeating distance texture for each evaluated line dash pattern.
use crate::style::{
    layer::LayerPaint,
    property::{NumberList, StyleProperty},
    Style,
};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const WIDTH: u32 = 256;
struct DashEntry {
    pattern: Vec<f64>,
    binding: wgpu::BindGroup,
}
pub(crate) struct LineDashResources {
    pub(crate) layout: wgpu::BindGroupLayout,
    entries: HashMap<String, DashEntry>,
    solid: DashEntry,
}

impl LineDashResources {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("line dash layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let solid = create_entry(device, queue, &layout, &[]);
        Self {
            layout,
            entries: HashMap::new(),
            solid,
        }
    }

    pub(crate) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        style: &Style,
        zoom: f64,
    ) {
        self.entries
            .retain(|id, _| style.layers.iter().any(|layer| &layer.id == id));
        for layer in &style.layers {
            let Some(LayerPaint::Line(paint)) = &layer.paint else {
                continue;
            };
            let pattern = paint
                .line_dasharray
                .as_ref()
                .and_then(|value| {
                    StyleProperty::<NumberList>::parse(value).evaluate_at_zoom(zoom.floor())
                })
                .map_or_else(Vec::new, |values| normalize_pattern(values.0));
            if pattern.is_empty() {
                self.entries.remove(&layer.id);
                continue;
            }
            if self
                .entries
                .get(&layer.id)
                .is_some_and(|entry| entry.pattern == pattern)
            {
                continue;
            }
            self.entries.insert(
                layer.id.clone(),
                create_entry(device, queue, &self.layout, &pattern),
            );
        }
    }

    pub(crate) fn binding(&self, layer: &str) -> &wgpu::BindGroup {
        &self.entries.get(layer).unwrap_or(&self.solid).binding
    }
}

fn normalize_pattern(mut pattern: Vec<f64>) -> Vec<f64> {
    if pattern.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Vec::new();
    }
    if pattern.len() % 2 != 0 {
        pattern.extend_from_within(..);
    }
    if pattern.iter().sum::<f64>() <= 0.0 {
        pattern.clear();
    }
    pattern
}

fn dash_pixels(pattern: &[f64]) -> (Vec<u8>, f32) {
    let period = pattern.iter().sum::<f64>();
    if period <= 0.0 {
        return (vec![255; WIDTH as usize * 4], 0.0);
    }
    let mut pixels = Vec::with_capacity(WIDTH as usize * 4);
    for x in 0..WIDTH {
        let position = (f64::from(x) + 0.5) / f64::from(WIDTH) * period;
        let mut edge = 0.0;
        let mut distance = period;
        let mut visible = true;
        for (index, length) in pattern.iter().enumerate() {
            if position >= edge && position < edge + length {
                visible = index % 2 == 0;
            }
            let delta = (position - edge).abs();
            distance = distance.min(delta.min(period - delta));
            edge += length;
        }
        let value = (128.0 + distance / period * 254.0 * if visible { 1.0 } else { -1.0 })
            .clamp(0.0, 255.0) as u8;
        pixels.extend([value, value, value, 255]);
    }
    (pixels, period as f32)
}

fn create_entry(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    pattern: &[f64],
) -> DashEntry {
    let (pixels, period) = dash_pixels(pattern);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("line dash distance"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: 1,
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
        &pixels,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: Some(1),
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
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("line dash period"),
        contents: bytemuck::cast_slice(&[period, 0.0_f32, 0.0, 0.0]),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("line dash"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    DashEntry {
        pattern: pattern.to_vec(),
        binding,
    }
}

#[cfg(test)]
mod tests;

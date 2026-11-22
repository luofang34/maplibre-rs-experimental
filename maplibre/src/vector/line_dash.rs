//! A small repeating distance texture for each evaluated line dash pattern.
use rustc_hash::{FxHashMap, FxHashSet};

use crate::style::{
    layer::LayerPaint,
    property::{NumberList, StyleProperty},
    Style, StyleLayerId,
};

mod image;
mod pixels;
use image::{image_uniform, line_image, pattern_image, ramp_texture, LineImage};
use pixels::{dash_pixels, dash_scale, normalize_pattern, round_dash_pixels, WIDTH};

struct DashEntry {
    pattern: Vec<f64>,
    /// Whether the dashes end in round caps, which the texture then encodes across the line.
    round: bool,
    image: LineImage,
    binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    dash_texture: wgpu::Texture,
    ramp_texture: wgpu::Texture,
    period: f32,
    /// The fractional-zoom scale of the dashes the uniform holds.
    scale: f32,
}

impl Drop for DashEntry {
    fn drop(&mut self) {
        self.uniform.destroy();
        self.dash_texture.destroy();
        self.ramp_texture.destroy();
    }
}
pub(crate) struct LineDashResources {
    pub(crate) layout: wgpu::BindGroupLayout,
    entries: FxHashMap<StyleLayerId, DashEntry>,
    solid: DashEntry,
    /// Layers whose pattern varies by feature, and the entry of each style image by its key.
    per_feature: FxHashSet<StyleLayerId>,
    images: FxHashMap<u32, DashEntry>,
    last_update: Option<(u64, usize, u32)>,
}

impl LineDashResources {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
    ) -> Self {
        let layout = crate::render::resource::create_bind_group_layout_cached(
            device,
            Some("line dash layout"),
            &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        );
        let solid = create_entry(
            device,
            queue,
            &layout,
            (&[], false),
            (&LineImage::None, 1.0),
        );
        Self {
            layout,
            entries: FxHashMap::default(),
            solid,
            per_feature: FxHashSet::default(),
            images: FxHashMap::default(),
            last_update: None,
        }
    }

    pub(crate) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        style: &Style,
        zoom: f64,
    ) {
        let scale = dash_scale(zoom);
        let key = (style.revision(), style.images.len(), scale.to_bits());
        if self.last_update == Some(key) {
            return;
        }
        self.entries.retain(|id, entry| {
            let keep = style.layers.iter().any(|layer| layer.layer_id() == *id);
            if !keep {
                queue.cancel_staged_buffer(&entry.uniform);
            }
            keep
        });
        self.sync_per_feature_layers(style);
        self.update_images(device, queue, style, scale);
        for layer in &style.layers {
            let Some(LayerPaint::Line(paint)) = &layer.paint else {
                continue;
            };
            let layer_id = layer.layer_id();
            let pattern = paint
                .line_dasharray
                .as_ref()
                .and_then(|value| {
                    StyleProperty::<NumberList>::parse(value).evaluate_at_zoom(zoom.floor())
                })
                .map_or_else(Vec::new, |values| normalize_pattern(values.0));
            let image = line_image(paint, style, zoom);
            let round = !pattern.is_empty()
                && crate::style::line_stroke::LineStroke::of_layer(layer).cap
                    == crate::style::line_stroke::LineCap::Round;
            if pattern.is_empty() && image == LineImage::None {
                if let Some(old) = self.entries.remove(&layer_id) {
                    queue.cancel_staged_buffer(&old.uniform);
                }
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&layer_id).filter(|entry| {
                entry.pattern == pattern && entry.round == round && entry.image == image
            }) {
                if entry.scale != scale {
                    entry.scale = scale;
                    let uniform = image_uniform(entry.period, &entry.image, entry.round, scale);
                    queue.write_buffer(&entry.uniform, 0, bytemuck::cast_slice(&uniform));
                }
                continue;
            }
            if let Some(old) = self.entries.insert(
                layer_id,
                create_entry(
                    device,
                    queue,
                    &self.layout,
                    (&pattern, round),
                    (&image, scale),
                ),
            ) {
                queue.cancel_staged_buffer(&old.uniform);
            }
        }
        self.last_update = Some(key);
    }

    fn sync_per_feature_layers(&mut self, style: &Style) {
        self.per_feature.clear();
        self.per_feature.extend(
            style
                .layers
                .iter()
                .filter(|layer| {
                    layer
                        .paint
                        .as_ref()
                        .is_some_and(|paint| super::pattern::per_feature_pattern(paint).is_some())
                })
                .filter(|layer| matches!(layer.paint, Some(LayerPaint::Line(_))))
                .map(|layer| layer.layer_id()),
        );
    }

    /// Keeps an entry for every image of the style while a line layer picks its image per feature.
    fn update_images(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        style: &Style,
        scale: f32,
    ) {
        if self.per_feature.is_empty() {
            for (_, old) in self.images.drain() {
                queue.cancel_staged_buffer(&old.uniform);
            }
            return;
        }
        let mut keys = FxHashSet::default();
        for (name, image) in &style.images {
            let key = crate::style::pattern_key::pattern_key(name);
            keys.insert(key);
            let Some(line_image) = pattern_image(image) else {
                if let Some(old) = self.images.remove(&key) {
                    queue.cancel_staged_buffer(&old.uniform);
                }
                continue;
            };
            if let Some(entry) = self
                .images
                .get_mut(&key)
                .filter(|entry| entry.image == line_image)
            {
                if entry.scale != scale {
                    entry.scale = scale;
                    let uniform = image_uniform(entry.period, &entry.image, false, scale);
                    queue.write_buffer(&entry.uniform, 0, bytemuck::cast_slice(&uniform));
                }
                continue;
            }
            let entry = create_entry(
                device,
                queue,
                &self.layout,
                (&[], false),
                (&line_image, scale),
            );
            if let Some(old) = self.images.insert(key, entry) {
                queue.cancel_staged_buffer(&old.uniform);
            }
        }
        self.images.retain(|key, old| {
            let keep = keys.contains(key);
            if !keep {
                queue.cancel_staged_buffer(&old.uniform);
            }
            keep
        });
    }

    /// The image a pattern key names, for a layer that picks its image per feature.
    pub(crate) fn image_binding(&self, key: u32) -> Option<&wgpu::BindGroup> {
        self.images.get(&key).map(|entry| &entry.binding)
    }

    #[inline]
    pub(crate) fn binding(&self, layer: StyleLayerId) -> &wgpu::BindGroup {
        &self.entries.get(&layer).unwrap_or(&self.solid).binding
    }

    #[inline]
    pub(crate) fn is_plain_solid(&self, layer: StyleLayerId) -> bool {
        !self.entries.contains_key(&layer) && !self.per_feature.contains(&layer)
    }
}

fn create_dash_texture_view(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
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

fn create_entry(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    layout: &wgpu::BindGroupLayout,
    (pattern, round): (&[f64], bool),
    (image, scale): (&LineImage, f32),
) -> DashEntry {
    let (pixels, period, rows) = if round {
        round_dash_pixels(pattern)
    } else {
        let (pixels, period) = dash_pixels(pattern);
        (pixels, period, 1)
    };
    let (dash_texture, view, sampler) = create_dash_texture_view(device, queue, &pixels, rows);
    let uniform = queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("line dash period"),
            contents: bytemuck::cast_slice(&image_uniform(period, image, round, scale)),
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
        pattern: pattern.to_vec(),
        round,
        image: image.clone(),
        binding,
        uniform,
        dash_texture,
        ramp_texture,
        period,
        scale,
    }
}

#[cfg(test)]
mod tests;

//! A small repeating distance texture for each evaluated line dash pattern.
use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::style::{
    layer::LayerPaint,
    property::{NumberList, StyleProperty},
    Style,
};

const WIDTH: u32 = 256;
/// What fills a line instead of its colour, when something does.
#[derive(Clone, Debug, Default, PartialEq)]
enum LineImage {
    /// The line takes its colour from the layer.
    #[default]
    None,
    /// Colours along the line, from `line-gradient`.
    Gradient(Vec<[u8; 4]>),
    /// An image repeated along the line, from `line-pattern`: its size in layout pixels and its
    /// RGBA bytes.
    Pattern {
        width: u32,
        height: u32,
        display: [f32; 2],
        data: Vec<u8>,
    },
}

struct DashEntry {
    pattern: Vec<f64>,
    /// Whether the dashes end in round caps, which the texture then encodes across the line.
    round: bool,
    image: LineImage,
    binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    period: f32,
    /// The fractional-zoom scale of the dashes the uniform holds.
    scale: f32,
}
pub(crate) struct LineDashResources {
    pub(crate) layout: wgpu::BindGroupLayout,
    entries: HashMap<String, DashEntry>,
    solid: DashEntry,
    /// Layers whose pattern varies by feature, and the entry of each style image by its key.
    per_feature: std::collections::HashSet<String>,
    images: HashMap<u32, DashEntry>,
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
        });
        let solid = create_entry(
            device,
            queue,
            &layout,
            (&[], false),
            (&LineImage::None, 1.0),
        );
        Self {
            layout,
            entries: HashMap::new(),
            solid,
            per_feature: Default::default(),
            images: HashMap::new(),
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
        let scale = dash_scale(zoom);
        self.per_feature = style
            .layers
            .iter()
            .filter(|layer| {
                layer
                    .paint
                    .as_ref()
                    .is_some_and(|paint| super::pattern::per_feature_pattern(paint).is_some())
            })
            .filter(|layer| matches!(layer.paint, Some(LayerPaint::Line(_))))
            .map(|layer| layer.id.clone())
            .collect();
        self.update_images(device, queue, style, scale);
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
            let image = line_image(paint, style, zoom);
            let round = !pattern.is_empty()
                && crate::style::line_stroke::LineStroke::of_layer(layer).cap
                    == crate::style::line_stroke::LineCap::Round;
            if pattern.is_empty() && image == LineImage::None {
                self.entries.remove(&layer.id);
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&layer.id).filter(|entry| {
                entry.pattern == pattern && entry.round == round && entry.image == image
            }) {
                if entry.scale != scale {
                    entry.scale = scale;
                    let uniform = image_uniform(entry.period, &entry.image, entry.round, scale);
                    queue.write_buffer(&entry.uniform, 0, bytemuck::cast_slice(&uniform));
                }
                continue;
            }
            self.entries.insert(
                layer.id.clone(),
                create_entry(
                    device,
                    queue,
                    &self.layout,
                    (&pattern, round),
                    (&image, scale),
                ),
            );
        }
    }

    /// Keeps an entry for every image of the style while a line layer picks its image per feature.
    fn update_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        style: &Style,
        scale: f32,
    ) {
        if self.per_feature.is_empty() {
            self.images.clear();
            return;
        }
        let mut keys = std::collections::HashSet::new();
        for (name, image) in &style.images {
            let key = crate::style::pattern_key::pattern_key(name);
            keys.insert(key);
            let Some(line_image) = pattern_image(image) else {
                self.images.remove(&key);
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
            self.images.insert(key, entry);
        }
        self.images.retain(|key, _| keys.contains(key));
    }

    /// The image a pattern key names, for a layer that picks its image per feature.
    pub(crate) fn image_binding(&self, key: u32) -> Option<&wgpu::BindGroup> {
        self.images.get(&key).map(|entry| &entry.binding)
    }

    pub(crate) fn binding(&self, layer: &str) -> &wgpu::BindGroup {
        &self.entries.get(layer).unwrap_or(&self.solid).binding
    }
}

/// The pattern as GL JS's line atlas reads it: an odd-length array joins its last dash to its
/// first, and a gap of zero length joins the dashes either side of it, so a pattern with no
/// real gap is a solid line.
fn normalize_pattern(pattern: Vec<f64>) -> Vec<f64> {
    if pattern.iter().any(|v| !v.is_finite() || *v < 0.0) || pattern.len() < 2 {
        return Vec::new();
    }
    let mut parts = pattern;
    if parts.len() % 2 == 1 {
        let last = parts.pop().unwrap_or(0.0);
        parts[0] += last;
    }
    let mut merged = vec![parts[0]];
    for pair in parts[1..].chunks(2) {
        match pair {
            [gap, dash] if *gap == 0.0 => {
                if let Some(last) = merged.last_mut() {
                    *last += dash;
                }
            }
            [gap, dash] => merged.extend([*gap, *dash]),
            [gap] if *gap > 0.0 => merged.push(*gap),
            _ => {}
        }
    }
    if merged.len() % 2 == 1 {
        if merged.len() == 1 {
            return Vec::new();
        }
        // The pattern ended on a dash that the first one continues.
        let last = merged.pop().unwrap_or(0.0);
        merged[0] += last;
    }
    if merged.iter().sum::<f64>() <= 0.0 {
        merged.clear();
    }
    merged
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

/// Rows either side of the centre of a round dash's texture.
const ROUND_HALF_ROWS: i32 = 7;

/// The dash texture with round caps: one row per step across the line, each holding the signed
/// distance to the dash shape at that height, so a dash and a gap end in half circles.
fn round_dash_pixels(pattern: &[f64]) -> (Vec<u8>, f32, u32) {
    let period = pattern.iter().sum::<f64>();
    let rows = (2 * ROUND_HALF_ROWS + 1) as u32;
    if period <= 0.0 {
        return (vec![255; WIDTH as usize * rows as usize * 4], 0.0, rows);
    }
    let stretch = f64::from(WIDTH) / period;
    let half = stretch / 2.0;
    let mut bounds = Vec::with_capacity(pattern.len());
    let mut edge = 0.0;
    for length in pattern {
        bounds.push((edge * stretch, (edge + length) * stretch));
        edge += length;
    }
    let mut pixels = Vec::with_capacity(WIDTH as usize * rows as usize * 4);
    for row in -ROUND_HALF_ROWS..=ROUND_HALF_ROWS {
        let middle = f64::from(row) / f64::from(ROUND_HALF_ROWS) * (half + 1.0);
        for x in 0..WIDTH {
            let x = f64::from(x);
            let (index, (left, right)) = bounds
                .iter()
                .copied()
                .enumerate()
                .find(|(_, (_, right))| x <= *right)
                .unwrap_or((bounds.len() - 1, bounds[bounds.len() - 1]));
            let nearest = (x - left).abs().min((x - right).abs());
            let signed = if index % 2 == 0 {
                (nearest * nearest + (half - middle.abs()).powi(2)).sqrt()
            } else {
                half - (nearest * nearest + middle * middle).sqrt()
            };
            let value = (128.0 + signed / f64::from(WIDTH) * 254.0).clamp(0.0, 255.0) as u8;
            pixels.extend([value, value, value, 255]);
        }
    }
    (pixels, period as f32, rows)
}

/// Dashes are laid out in tile units of the whole zoom below the view's, as GL JS does, so
/// between whole zooms they grow with the map instead of holding their size on screen.
fn dash_scale(zoom: f64) -> f32 {
    2.0_f64.powf(zoom - zoom.floor()) as f32
}

fn create_entry(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
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
        &pixels,
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
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("line dash period"),
        contents: bytemuck::cast_slice(&image_uniform(period, image, round, scale)),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let (ramp_view, ramp_sampler) = ramp_texture(device, queue, image);
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
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&ramp_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&ramp_sampler),
            },
        ],
    });
    DashEntry {
        pattern: pattern.to_vec(),
        round,
        image: image.clone(),
        binding,
        uniform,
        period,
        scale,
    }
}

/// The uniform of a dash entry: the dash period, what fills the line, and the size of a pattern
/// or, for a dash, whether its texture has the rows of round caps and how far the fractional
/// zoom stretches it.
fn image_uniform(period: f32, image: &LineImage, round: bool, scale: f32) -> [f32; 4] {
    let round = f32::from(round);
    match image {
        LineImage::None => [period, 0.0, round, scale],
        LineImage::Gradient(_) => [period, 1.0, round, scale],
        LineImage::Pattern { display, .. } => [period, 2.0, display[0], display[1]],
    }
}

/// The image a line layer draws instead of its colour at a zoom.
fn line_image(paint: &crate::style::layer::LinePaint, style: &Style, zoom: f64) -> LineImage {
    if let Some(gradient) = &paint.line_gradient {
        return LineImage::Gradient(crate::style::line_gradient::ramp(gradient));
    }
    let Some(pattern) = &paint.line_pattern else {
        return LineImage::None;
    };
    let name = StyleProperty::<crate::style::layer::TextField>::parse(pattern)
        .evaluate_at_zoom(zoom)
        .map(|name| name.0);
    name.and_then(|name| style.images.get(&name))
        .and_then(pattern_image)
        .unwrap_or(LineImage::None)
}

/// The pattern an image makes, when its pixels fit its size.
fn pattern_image(image: &crate::style::StyleImage) -> Option<LineImage> {
    if image.width == 0
        || image.height == 0
        || image.data.len() != image.width as usize * image.height as usize * 4
    {
        return None;
    }
    let ratio = image.pixel_ratio.max(0.01);
    Some(LineImage::Pattern {
        width: image.width,
        height: image.height,
        display: [image.width as f32 / ratio, image.height as f32 / ratio],
        data: super::pattern::premultiplied(&image.data),
    })
}

/// The gradient ramp or pattern as a texture, or one white texel for a plain line.
fn ramp_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &LineImage,
) -> (wgpu::TextureView, wgpu::Sampler) {
    let white = [255_u8; 4];
    let (width, height, bytes): (u32, u32, &[u8]) = match image {
        LineImage::None => (1, 1, &white),
        LineImage::Gradient(texels) => (texels.len() as u32, 1, bytemuck::cast_slice(texels)),
        LineImage::Pattern {
            width,
            height,
            data,
            ..
        } => (*width, *height, data),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("line image"),
        size: wgpu::Extent3d {
            width,
            height,
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
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        texture.size(),
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (texture.create_view(&Default::default()), sampler)
}

#[cfg(test)]
mod tests;

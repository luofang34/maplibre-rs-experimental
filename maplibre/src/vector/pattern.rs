//! The images that `fill-pattern` layers repeat, and the bindings that draw them.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use wgpu::util::DeviceExt;

use crate::style::{
    layer::{LayerPaint, StyleProperty, TextField},
    Style, StyleImage,
};

struct PatternBinding {
    /// Identity of the image the binding was made for, so a replaced image is uploaded again.
    fingerprint: u64,
    bind_group: wgpu::BindGroup,
}

/// The pipeline and the per-layer bindings of layers that fill with a repeating image.
pub(crate) struct PatternResources {
    /// The fill pipeline, once a vector plugin has built it; backgrounds need only the images.
    pipeline: Option<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    layers: HashMap<String, PatternBinding>,
    sampler: wgpu::Sampler,
}

/// Bind group layout of one layer's pattern: the image size, the image and its sampler.
pub(crate) fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// The name of the image a fill, extrusion or background layer repeats at a zoom, if it repeats one.
pub(crate) fn pattern_name(paint: &LayerPaint, zoom: f64) -> Option<String> {
    let value = match paint {
        LayerPaint::Fill(fill) => fill.fill_pattern.as_ref()?,
        LayerPaint::FillExtrusion(extrusion) => extrusion.fill_extrusion_pattern.as_ref()?,
        LayerPaint::Background(background) => background.background_pattern.as_ref()?,
        _ => return None,
    };
    StyleProperty::<TextField>::parse(value)
        .evaluate_at_zoom(zoom)
        .map(|name| name.0)
        .filter(|name| !name.is_empty())
}

fn fingerprint(name: &str, image: &StyleImage) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    name.hash(&mut hasher);
    (image.width, image.height, image.sdf).hash(&mut hasher);
    image.pixel_ratio.to_bits().hash(&mut hasher);
    image.data.hash(&mut hasher);
    hasher.finish()
}

impl PatternResources {
    /// Creates the bindings, whose layout is [`layout_entries`], without a fill pipeline.
    pub(crate) fn new(device: &wgpu::Device, layout: wgpu::BindGroupLayout) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline: None,
            layout,
            layers: HashMap::new(),
            sampler,
        }
    }

    /// Binds the image each pattern layer names at `zoom`, dropping layers that no longer
    /// repeat an image the style holds.
    pub(crate) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        style: &Style,
        zoom: f64,
    ) {
        self.layers
            .retain(|id, _| style.layers.iter().any(|layer| &layer.id == id));
        for layer in &style.layers {
            let image = layer
                .paint
                .as_ref()
                .and_then(|paint| pattern_name(paint, zoom))
                .and_then(|name| style.images.get(&name).map(|image| (name, image)));
            let Some((name, image)) = image else {
                self.layers.remove(&layer.id);
                continue;
            };
            let fingerprint = fingerprint(&name, image);
            if self
                .layers
                .get(&layer.id)
                .is_some_and(|binding| binding.fingerprint == fingerprint)
            {
                continue;
            }
            if let Some(bind_group) = self.bind(device, queue, image) {
                self.layers.insert(
                    layer.id.clone(),
                    PatternBinding {
                        fingerprint,
                        bind_group,
                    },
                );
            }
        }
    }

    fn bind(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &StyleImage,
    ) -> Option<wgpu::BindGroup> {
        if image.width == 0
            || image.height == 0
            || image.data.len() != image.width as usize * image.height as usize * 4
        {
            return None;
        }
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("fill pattern"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &image.data,
        );
        let ratio = image.pixel_ratio.max(0.01);
        let size = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fill pattern size"),
            contents: bytemuck::cast_slice(&[
                image.width as f32 / ratio,
                image.height as f32 / ratio,
                0.0_f32,
                0.0,
            ]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = texture.create_view(&Default::default());
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fill pattern"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: size.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }))
    }

    /// Sets the pipeline that draws fills with these images.
    pub(crate) fn set_pipeline(&mut self, pipeline: wgpu::RenderPipeline) {
        self.pipeline = Some(pipeline);
    }

    pub(crate) fn pipeline(&self) -> Option<&wgpu::RenderPipeline> {
        self.pipeline.as_ref()
    }

    pub(crate) fn binding(&self, layer: &str) -> Option<&wgpu::BindGroup> {
        self.layers.get(layer).map(|binding| &binding.bind_group)
    }
}

#[cfg(test)]
mod tests;

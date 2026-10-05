//! The images that `fill-pattern` layers repeat, and the bindings that draw them.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use crate::style::{
    layer::{ImageName, LayerPaint, StyleProperty},
    Style, StyleImage,
};

struct PatternBinding {
    /// Identity of the image the binding was made for, so a replaced image is uploaded again.
    fingerprint: u64,
    bind_group: wgpu::BindGroup,
    size: wgpu::Buffer,
    /// The image's display size in layout pixels.
    display: [f32; 2],
    /// How far the fractional zoom stretches the pattern the buffer holds.
    scale: f32,
}

/// The pipeline and the per-layer bindings of layers that fill with a repeating image.
pub(crate) struct PatternResources {
    /// The fill pipeline, once a vector plugin has built it; backgrounds need only the images.
    pipeline: Option<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    layers: HashMap<String, PatternBinding>,
    /// Layers whose pattern varies by feature, and so bind an image per run of features.
    per_feature: std::collections::HashSet<String>,
    /// Every image of the style by its pattern key, for the layers in `per_feature`.
    images: HashMap<u32, PatternBinding>,
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
    StyleProperty::<ImageName>::parse(value)
        .evaluate_at_zoom(zoom)
        .map(|name| name.0)
        .filter(|name| !name.is_empty())
}

/// The pattern property of a fill, extrusion or line layer when its image varies by feature.
pub(crate) fn per_feature_pattern(paint: &LayerPaint) -> Option<StyleProperty<ImageName>> {
    let value = match paint {
        LayerPaint::Fill(fill) => fill.fill_pattern.as_ref()?,
        LayerPaint::FillExtrusion(extrusion) => extrusion.fill_extrusion_pattern.as_ref()?,
        LayerPaint::Line(line) => line.line_pattern.as_ref()?,
        _ => return None,
    };
    let property = StyleProperty::<ImageName>::parse(value);
    (!property.is_feature_constant()).then_some(property)
}

/// Whether a layer declares a pattern that names no image the style holds at `zoom`, so that GL
/// JS draws nothing for it; a name that varies by feature counts as none here.
pub(crate) fn names_missing_image(paint: Option<&LayerPaint>, style: &Style, zoom: f64) -> bool {
    let value = match paint {
        Some(LayerPaint::Fill(fill)) => fill.fill_pattern.as_ref(),
        Some(LayerPaint::FillExtrusion(extrusion)) => extrusion.fill_extrusion_pattern.as_ref(),
        Some(LayerPaint::Background(background)) => background.background_pattern.as_ref(),
        _ => None,
    };
    value.is_some_and(|value| {
        let property = StyleProperty::<ImageName>::parse(value);
        // A feature's own name decides, where its geometry is drawn.
        property.is_feature_constant()
            && property
                .evaluate_at_zoom(zoom)
                .is_none_or(|name| !style.images.contains_key(&name.0))
    })
}

/// Patterns are laid out in tile units of the whole zoom below the view's, as GL JS does, so
/// between whole zooms they grow with the map.
fn pattern_scale(zoom: f64) -> f32 {
    2.0_f64.powf(zoom - zoom.floor()) as f32
}

/// The image's colours multiplied by its alpha, which filters without a dark fringe.
pub(super) fn premultiplied(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    for texel in out.as_chunks_mut::<4>().0 {
        let alpha = u16::from(texel[3]);
        for channel in &mut texel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    out
}

fn size(display: [f32; 2], scale: f32) -> [f32; 4] {
    [display[0] * scale, display[1] * scale, 0.0, 0.0]
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
            per_feature: Default::default(),
            images: HashMap::new(),
            sampler,
        }
    }

    /// Binds the image each pattern layer names at `zoom`, dropping layers that no longer
    /// repeat an image the style holds.
    pub(crate) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        style: &Style,
        zoom: f64,
    ) {
        let scale = pattern_scale(zoom);
        self.layers
            .retain(|id, _| style.layers.iter().any(|layer| &layer.id == id));
        self.per_feature = style
            .layers
            .iter()
            .filter(|layer| layer.paint.as_ref().and_then(per_feature_pattern).is_some())
            .map(|layer| layer.id.clone())
            .collect();
        self.update_images(device, queue, style, scale);
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
            if let Some(binding) = self
                .layers
                .get_mut(&layer.id)
                .filter(|binding| binding.fingerprint == fingerprint)
            {
                if binding.scale != scale {
                    binding.scale = scale;
                    queue.write_buffer(
                        &binding.size,
                        0,
                        bytemuck::cast_slice(&size(binding.display, scale)),
                    );
                }
                continue;
            }
            if let Some(binding) = self.bind(device, queue, image, (fingerprint, scale)) {
                self.layers.insert(layer.id.clone(), binding);
            }
        }
    }

    /// Keeps a binding for every image of the style while a layer picks its image per feature.
    fn update_images(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
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
            let fingerprint = fingerprint(name, image);
            if let Some(binding) = self
                .images
                .get_mut(&key)
                .filter(|binding| binding.fingerprint == fingerprint)
            {
                if binding.scale != scale {
                    binding.scale = scale;
                    queue.write_buffer(
                        &binding.size,
                        0,
                        bytemuck::cast_slice(&size(binding.display, scale)),
                    );
                }
                continue;
            }
            match self.bind(device, queue, image, (fingerprint, scale)) {
                Some(binding) => {
                    self.images.insert(key, binding);
                }
                None => {
                    self.images.remove(&key);
                }
            }
        }
        self.images.retain(|key, _| keys.contains(key));
    }

    /// Whether the layer picks its image per feature.
    pub(crate) fn is_per_feature(&self, layer: &str) -> bool {
        self.per_feature.contains(layer)
    }

    /// The image a pattern key names, for a layer that picks its image per feature.
    pub(crate) fn image_binding(&self, key: u32) -> Option<&wgpu::BindGroup> {
        self.images.get(&key).map(|binding| &binding.bind_group)
    }

    fn bind(
        &self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        image: &StyleImage,
        (fingerprint, scale): (u64, f32),
    ) -> Option<PatternBinding> {
        if image.width == 0
            || image.height == 0
            || image.data.len() != image.width as usize * image.height as usize * 4
        {
            return None;
        }
        let texture = queue.create_texture_with_data(
            device,
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
            &premultiplied(&image.data),
        );
        let ratio = image.pixel_ratio.max(0.01);
        let display = [image.width as f32 / ratio, image.height as f32 / ratio];
        let size = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("fill pattern size"),
                contents: bytemuck::cast_slice(&size(display, scale)),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        });
        Some(PatternBinding {
            fingerprint,
            bind_group,
            size,
            display,
            scale,
        })
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

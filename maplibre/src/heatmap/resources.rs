//! GPU resources of heatmap layers: two pipelines, and per layer a density target, a colour
//! ramp texture and an opacity uniform.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

use crate::{render::shaders::DENSITY_FORMAT, style::heatmap::RAMP_TEXELS};

struct LayerTargets {
    size: (u32, u32),
    density_view: wgpu::TextureView,
    density_bind_group: wgpu::BindGroup,
    ramp: wgpu::Texture,
    ramp_fingerprint: u64,
    opacity: wgpu::Buffer,
    ramp_bind_group: wgpu::BindGroup,
}

/// The density and composite pipelines, and the targets of every visible heatmap layer.
pub struct HeatmapResources {
    density_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layers: HashMap<String, LayerTargets>,
}

impl HeatmapResources {
    /// Wraps the pipelines. The density pipeline takes the projection at group 0; the composite
    /// takes the density texture and sampler at group 0 and the ramp and opacity at group 1.
    pub fn new(
        device: &wgpu::Device,
        density_pipeline: wgpu::RenderPipeline,
        composite_pipeline: wgpu::RenderPipeline,
    ) -> Self {
        // Densities are looked up at arbitrary positions, and the ramp must not wrap.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            density_pipeline,
            composite_pipeline,
            sampler,
            layers: HashMap::new(),
        }
    }

    /// The pipeline that adds kernels into a density target.
    pub fn density_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.density_pipeline
    }

    /// The pipeline that colours a density target into the main pass.
    pub fn composite_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.composite_pipeline
    }

    /// Makes a layer's targets match the viewport, its ramp and its opacity.
    ///
    /// The density texture is recreated when the viewport changes size and the ramp texture
    /// is rewritten only when its contents change.
    pub fn write_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: &str,
        size: (u32, u32),
        ramp: &[[u8; 4]],
        opacity: f32,
    ) {
        let mut hasher = std::hash::DefaultHasher::new();
        ramp.hash(&mut hasher);
        let ramp_fingerprint = hasher.finish();
        if self
            .layers
            .get(layer_id)
            .is_none_or(|targets| targets.size != size)
        {
            let targets = self.create_targets(device, size);
            self.layers.insert(layer_id.to_string(), targets);
        }
        let Some(targets) = self.layers.get_mut(layer_id) else {
            return;
        };
        if targets.ramp_fingerprint != ramp_fingerprint || targets.ramp_fingerprint == 0 {
            write_ramp(queue, &targets.ramp, ramp);
            targets.ramp_fingerprint = ramp_fingerprint;
        }
        let uniform = [opacity, 0.0, 0.0, 0.0];
        queue.write_buffer(&targets.opacity, 0, bytemuck::bytes_of(&uniform));
    }

    fn create_targets(&self, device: &wgpu::Device, size: (u32, u32)) -> LayerTargets {
        let density = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("heatmap density"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DENSITY_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let density_view = density.create_view(&Default::default());
        let density_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("heatmap density"),
            layout: &self.composite_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&density_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let ramp = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("heatmap ramp"),
            size: wgpu::Extent3d {
                width: RAMP_TEXELS as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The ramp is looked up in the encoded colour space, as other imagery is.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let opacity = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("heatmap opacity"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ramp_view = ramp.create_view(&Default::default());
        let ramp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("heatmap ramp"),
            layout: &self.composite_pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: opacity.as_entire_binding(),
                },
            ],
        });
        LayerTargets {
            size,
            density_view,
            density_bind_group,
            ramp,
            ramp_fingerprint: 0,
            opacity,
            ramp_bind_group,
        }
    }

    /// The view the density pass renders into.
    pub fn density_view(&self, layer_id: &str) -> Option<&wgpu::TextureView> {
        self.layers
            .get(layer_id)
            .map(|targets| &targets.density_view)
    }

    /// The density texture and sampler of a layer, bound at group 0 of the composite.
    pub fn density_bind_group(&self, layer_id: &str) -> Option<&wgpu::BindGroup> {
        self.layers
            .get(layer_id)
            .map(|targets| &targets.density_bind_group)
    }

    /// The ramp and opacity of a layer, bound at group 1 of the composite.
    pub fn ramp_bind_group(&self, layer_id: &str) -> Option<&wgpu::BindGroup> {
        self.layers
            .get(layer_id)
            .map(|targets| &targets.ramp_bind_group)
    }

    /// Drops the targets of layers that are no longer visible or no longer in the style.
    pub fn retain_layers(&mut self, layer_ids: &HashSet<&str>) {
        self.layers.retain(|id, _| layer_ids.contains(id.as_str()));
    }
}

fn write_ramp(queue: &wgpu::Queue, texture: &wgpu::Texture, ramp: &[[u8; 4]]) {
    let mut texels = Vec::with_capacity(RAMP_TEXELS * 4);
    for texel in ramp.iter().take(RAMP_TEXELS) {
        texels.extend_from_slice(texel);
    }
    texels.resize(RAMP_TEXELS * 4, 0);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(RAMP_TEXELS as u32 * 4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: RAMP_TEXELS as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}

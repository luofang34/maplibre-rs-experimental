//! GPU resources of the DEM-shaded layers: the two pipelines and one uniform buffer per layer.

use std::collections::{HashMap, HashSet};

use bytemuck_derive::{Pod, Zeroable};

use crate::style::hillshade::{Illumination, MAX_LIGHTS, MAX_RAMP_STOPS};

/// Which shading a layer uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemLayerKind {
    /// A `hillshade` layer.
    Hillshade,
    /// A `color-relief` layer.
    ColorRelief,
}

/// Per-layer values of the hillshade shader; the layout mirrors the WGSL struct.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct HillshadeUniforms {
    /// DEM unpack vector of the source.
    pub unpack: [f32; 4],
    /// Premultiplied accent colour.
    pub accent: [f32; 4],
    /// Premultiplied shadow colour per light.
    pub shadows: [[f32; 4]; MAX_LIGHTS],
    /// Premultiplied highlight colour per light.
    pub highlights: [[f32; 4]; MAX_LIGHTS],
    /// Light altitudes in radians.
    pub altitudes: [f32; MAX_LIGHTS],
    /// Light azimuths in radians.
    pub azimuths: [f32; MAX_LIGHTS],
    /// Shading intensity.
    pub exaggeration: f32,
    /// Shading method code.
    pub method: u32,
    /// Number of lights in use.
    pub light_count: u32,
    /// Keeps the struct a multiple of sixteen bytes.
    pub padding: u32,
}

impl HillshadeUniforms {
    /// Packs a layer's lights, accent and method for the shader.
    pub fn new(
        unpack: [f32; 4],
        illumination: &Illumination,
        accent: [f32; 4],
        exaggeration: f32,
        method: u32,
    ) -> Self {
        let mut uniforms = Self {
            unpack,
            accent,
            shadows: [[0.0; 4]; MAX_LIGHTS],
            highlights: [[0.0; 4]; MAX_LIGHTS],
            altitudes: [0.0; MAX_LIGHTS],
            azimuths: [0.0; MAX_LIGHTS],
            exaggeration,
            method,
            light_count: illumination.azimuths.len().min(MAX_LIGHTS) as u32,
            padding: 0,
        };
        for light in 0..uniforms.light_count as usize {
            uniforms.shadows[light] = illumination.shadows[light];
            uniforms.highlights[light] = illumination.highlights[light];
            uniforms.altitudes[light] = illumination.altitudes[light];
            uniforms.azimuths[light] = illumination.azimuths[light];
        }
        uniforms
    }
}

/// Per-layer values of the colour relief shader; the layout mirrors the WGSL struct.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ColorReliefUniforms {
    /// DEM unpack vector of the source.
    pub unpack: [f32; 4],
    /// Opacity of the relief.
    pub opacity: f32,
    /// Number of ramp stops in use.
    pub stop_count: u32,
    /// Keeps the arrays sixteen-byte aligned.
    pub padding: [u32; 2],
    /// Stop elevations, four per vector.
    pub elevations: [[f32; 4]; MAX_RAMP_STOPS / 4],
    /// Premultiplied stop colours.
    pub colors: [[f32; 4]; MAX_RAMP_STOPS],
}

impl ColorReliefUniforms {
    /// Packs a layer's ramp for the shader.
    pub fn new(unpack: [f32; 4], opacity: f32, ramp: &[(f32, [f32; 4])]) -> Self {
        let mut uniforms = Self {
            unpack,
            opacity,
            stop_count: ramp.len().min(MAX_RAMP_STOPS) as u32,
            padding: [0; 2],
            elevations: [[0.0; 4]; MAX_RAMP_STOPS / 4],
            colors: [[0.0; 4]; MAX_RAMP_STOPS],
        };
        for (index, (elevation, color)) in ramp.iter().take(MAX_RAMP_STOPS).enumerate() {
            uniforms.elevations[index / 4][index % 4] = *elevation;
            uniforms.colors[index] = *color;
        }
        uniforms
    }
}

struct LayerBinding {
    kind: DemLayerKind,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// The pipelines of both layer kinds and the uniform buffer of every DEM-shaded layer.
pub struct HillshadeResources {
    hillshade_pipeline: wgpu::RenderPipeline,
    relief_pipeline: wgpu::RenderPipeline,
    layers: HashMap<String, LayerBinding>,
}

impl HillshadeResources {
    /// Wraps the two pipelines; both take the projection at group 0, the tile texture at
    /// group 1 and the layer's uniforms at group 2.
    pub fn new(
        hillshade_pipeline: wgpu::RenderPipeline,
        relief_pipeline: wgpu::RenderPipeline,
    ) -> Self {
        Self {
            hillshade_pipeline,
            relief_pipeline,
            layers: HashMap::new(),
        }
    }

    /// The pipeline drawing a layer kind.
    pub fn pipeline(&self, kind: DemLayerKind) -> &wgpu::RenderPipeline {
        match kind {
            DemLayerKind::Hillshade => &self.hillshade_pipeline,
            DemLayerKind::ColorRelief => &self.relief_pipeline,
        }
    }

    /// Writes a layer's uniforms, creating its buffer on first use or when its size changes.
    pub fn write_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: &str,
        kind: DemLayerKind,
        contents: &[u8],
    ) {
        let reusable = self.layers.get(layer_id).is_some_and(|binding| {
            binding.kind == kind && binding.buffer.size() == contents.len() as u64
        });
        if !reusable {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("DEM layer uniforms"),
                size: contents.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("DEM layer uniforms"),
                layout: &self.pipeline(kind).get_bind_group_layout(2),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.layers.insert(
                layer_id.to_string(),
                LayerBinding {
                    kind,
                    buffer,
                    bind_group,
                },
            );
        }
        if let Some(binding) = self.layers.get(layer_id) {
            queue.write_buffer(&binding.buffer, 0, contents);
        }
    }

    /// Kind and uniform bind group of a layer written this frame.
    pub fn layer(&self, layer_id: &str) -> Option<(DemLayerKind, &wgpu::BindGroup)> {
        self.layers
            .get(layer_id)
            .map(|binding| (binding.kind, &binding.bind_group))
    }

    /// Drops the buffers of layers the style no longer has.
    pub fn retain_layers(&mut self, layer_ids: &HashSet<&str>) {
        self.layers.retain(|id, _| layer_ids.contains(id.as_str()));
    }
}

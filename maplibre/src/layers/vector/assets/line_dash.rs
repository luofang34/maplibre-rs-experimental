//! A small repeating distance texture for each evaluated line dash pattern.
use std::time::Duration;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    render::upload_queue::UploadQueue,
    style::{
        layer::{LayerPaint, LinePaint, StyleLayer},
        Style, StyleLayerId,
    },
};

mod crossfade;
mod entry;
mod image;
mod pixels;
use crossfade::{
    eval_floor_width, evaluate_dash_patterns, CrossfadeParameters, ZoomHistory,
    DEFAULT_FADE_DURATION,
};
use entry::{create_entry, DashEntry, DashSpec};
use image::{line_image, pattern_image, LineImage};
use pixels::dash_scale;
#[cfg(test)]
use pixels::{build_dash_texture_pixels, WIDTH};

const SETTLED_CROSSFADE: CrossfadeParameters = CrossfadeParameters {
    from_scale: 1.0,
    to_scale: 1.0,
    mix: 1.0,
    fade_t: 1.0,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct DashUpdateKey {
    style_rev: u64,
    images_len: usize,
    floor_z_bits: u32,
    scale_bits: u32,
    from_scale_bits: u32,
    to_scale_bits: u32,
    mix_bits: u32,
}

#[derive(Clone, Copy)]
struct DashFrameParams {
    zoom: f64,
    floor_z: f64,
    zooming_in: bool,
    scale: f32,
    crossfade: CrossfadeParameters,
}

pub(crate) struct LineDashResources {
    pub(crate) layout: wgpu::BindGroupLayout,
    entries: FxHashMap<StyleLayerId, DashEntry>,
    solid: DashEntry,
    /// Layers whose pattern varies by feature, and the entry of each style image by its key.
    per_feature: FxHashSet<StyleLayerId>,
    images: FxHashMap<u32, DashEntry>,
    zoom_history: ZoomHistory,
    last_update: Option<DashUpdateKey>,
    last_pattern_epoch: Option<(u64, usize, u32, bool)>,
    has_line_pattern_layer: bool,
}

impl LineDashResources {
    pub(crate) fn new(device: &wgpu::Device, queue: &UploadQueue) -> Self {
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
            DashSpec {
                from_pattern: &[],
                to_pattern: &[],
                round: false,
                floor_width: 1.0,
            },
            (&LineImage::None, 1.0, SETTLED_CROSSFADE),
        );
        Self {
            layout,
            entries: FxHashMap::default(),
            solid,
            per_feature: FxHashSet::default(),
            images: FxHashMap::default(),
            zoom_history: ZoomHistory::default(),
            last_update: None,
            last_pattern_epoch: None,
            has_line_pattern_layer: false,
        }
    }

    pub(crate) fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        style: &Style,
        zoom: f64,
        now: Option<Duration>,
    ) -> bool {
        self.zoom_history.update(zoom, now);
        let crossfade = self
            .zoom_history
            .crossfade(zoom, now, DEFAULT_FADE_DURATION);
        let scale = dash_scale(zoom);
        let floor_z = zoom.floor();
        let floor_z_bits = (floor_z as f32).to_bits();
        let key = DashUpdateKey {
            style_rev: style.revision(),
            images_len: style.images.len(),
            floor_z_bits,
            scale_bits: scale.to_bits(),
            from_scale_bits: crossfade.from_scale.to_bits(),
            to_scale_bits: crossfade.to_scale.to_bits(),
            mix_bits: crossfade.mix.to_bits(),
        };
        if self.last_update == Some(key) {
            return crossfade.fade_t < 1.0 && self.has_active_dash();
        }
        let zooming_in = self.zoom_history.zooming_in(zoom);
        let pattern_epoch = (
            style.revision(),
            style.images.len(),
            floor_z_bits,
            zooming_in,
        );
        if self.last_pattern_epoch == Some(pattern_epoch) && !self.has_line_pattern_layer {
            self.update_entries_uniforms_only(queue, scale, crossfade);
            self.last_update = Some(key);
            return crossfade.fade_t < 1.0 && self.has_active_dash();
        }
        let params = DashFrameParams {
            zoom,
            floor_z,
            zooming_in,
            scale,
            crossfade,
        };
        self.sync_entries_epoch(device, queue, style, params);
        self.last_pattern_epoch = Some(pattern_epoch);
        self.last_update = Some(key);
        crossfade.fade_t < 1.0 && self.has_active_dash()
    }

    fn update_entries_uniforms_only(
        &mut self,
        queue: &UploadQueue,
        scale: f32,
        crossfade: CrossfadeParameters,
    ) {
        for entry in self.entries.values_mut() {
            let floor_width = entry.floor_width;
            entry.refresh_uniform(queue, scale, crossfade, floor_width);
        }
        for entry in self.images.values_mut() {
            let floor_width = entry.floor_width;
            entry.refresh_uniform(queue, scale, crossfade, floor_width);
        }
    }

    fn sync_entries_epoch(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        style: &Style,
        params: DashFrameParams,
    ) {
        self.entries.retain(|id, entry| {
            let keep = style.layers.iter().any(|layer| layer.layer_id() == *id);
            if !keep {
                queue.cancel_staged_buffer(&entry.uniform);
            }
            keep
        });
        self.sync_per_feature_layers(style);
        self.has_line_pattern_layer = style.layers.iter().any(|layer| {
            matches!(
                &layer.paint,
                Some(LayerPaint::Line(paint)) if paint.line_pattern.is_some()
            )
        });
        self.update_images(device, queue, style, params.scale, params.crossfade);
        for layer in &style.layers {
            let Some(LayerPaint::Line(paint)) = &layer.paint else {
                continue;
            };
            self.sync_layer_entry(device, queue, style, (layer, paint), params);
        }
    }

    fn sync_layer_entry(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        style: &Style,
        (layer, paint): (&StyleLayer, &LinePaint),
        params: DashFrameParams,
    ) {
        let layer_id = layer.layer_id();
        let (from_pattern, to_pattern) =
            evaluate_dash_patterns(paint, params.floor_z, params.zooming_in);
        let image = line_image(paint, style, params.zoom);
        let floor_width = eval_floor_width(paint, params.zoom);
        let round = (!from_pattern.is_empty() || !to_pattern.is_empty())
            && crate::style::line_stroke::LineStroke::of_layer(layer).cap
                == crate::style::line_stroke::LineCap::Round;
        if from_pattern.is_empty() && to_pattern.is_empty() && image == LineImage::None {
            if let Some(old) = self.entries.remove(&layer_id) {
                queue.cancel_staged_buffer(&old.uniform);
            }
            return;
        }
        if let Some(entry) = self.entries.get_mut(&layer_id).filter(|entry| {
            entry.from_pattern == from_pattern
                && entry.to_pattern == to_pattern
                && entry.round == round
                && entry.image == image
        }) {
            entry.refresh_uniform(queue, params.scale, params.crossfade, floor_width);
            return;
        }
        let created = create_entry(
            device,
            queue,
            &self.layout,
            DashSpec {
                from_pattern: &from_pattern,
                to_pattern: &to_pattern,
                round,
                floor_width,
            },
            (&image, params.scale, params.crossfade),
        );
        if let Some(old) = self.entries.insert(layer_id, created) {
            queue.cancel_staged_buffer(&old.uniform);
        }
    }

    fn has_active_dash(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.to_period > 0.0 || entry.from_period > 0.0)
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
        queue: &UploadQueue,
        style: &Style,
        scale: f32,
        crossfade: CrossfadeParameters,
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
                entry.refresh_uniform(queue, scale, crossfade, -1.0);
                continue;
            }
            let entry = create_entry(
                device,
                queue,
                &self.layout,
                DashSpec {
                    from_pattern: &[],
                    to_pattern: &[],
                    round: false,
                    floor_width: -1.0,
                },
                (&line_image, scale, crossfade),
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

#[cfg(test)]
mod tests;

//! GPU atlases shared by layers and their evaluated draw uniforms.
use super::{assets::SymbolAtlas, paint::SymbolUniforms};
use crate::{coords::WorldTileCoords, style::layer::SymbolPaint};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use wgpu::util::{DeviceExt, TextureDataOrder};

struct TileAtlas {
    source: Arc<SymbolAtlas>,
    texture: wgpu::Texture,
}

struct DrawBinding {
    atlas: Arc<TileAtlas>,
    group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    uniforms: SymbolUniforms,
}

pub(super) struct TextureContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub pipeline: &'a wgpu::RenderPipeline,
}

#[derive(Default)]
pub(super) struct SymbolTextures {
    atlases: HashMap<usize, Weak<TileAtlas>>,
    bindings: HashMap<(WorldTileCoords, String), DrawBinding>,
}

impl SymbolTextures {
    pub fn prepare(
        &mut self,
        gpu: &TextureContext<'_>,
        key: (WorldTileCoords, String),
        atlas: &Arc<SymbolAtlas>,
        paint: &SymbolPaint,
        zoom: f64,
    ) {
        let uniforms = SymbolUniforms::new(paint, zoom, atlas.size);
        if let Some(binding) = self.bindings.get_mut(&key) {
            if Arc::ptr_eq(&binding.atlas.source, atlas) {
                if binding.uniforms != uniforms {
                    gpu.queue
                        .write_buffer(&binding.buffer, 0, bytemuck::bytes_of(&uniforms));
                    binding.uniforms = uniforms;
                }
                return;
            }
        }
        // Source atlases can share tile coordinates without sharing sprite or font pixels.
        let identity = Arc::as_ptr(atlas) as usize;
        let texture = self
            .atlases
            .get(&identity)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let texture = Arc::new(TileAtlas::new(gpu, atlas));
                self.atlases.insert(identity, Arc::downgrade(&texture));
                texture
            });
        self.bindings
            .insert(key, DrawBinding::new(gpu, texture, uniforms));
    }

    pub fn binding(&self, coords: WorldTileCoords, layer: &str) -> Option<&wgpu::BindGroup> {
        self.bindings
            .get(&(coords, layer.to_string()))
            .map(|binding| &binding.group)
    }

    pub fn retain(&mut self, tiles: &crate::tcs::tiles::Tiles) {
        self.bindings.retain(|(coords, _), _| {
            tiles
                .query::<&super::SymbolLayersDataComponent>(*coords)
                .is_some()
        });
        self.atlases.retain(|_, atlas| atlas.strong_count() > 0);
    }
}

impl TileAtlas {
    fn new(gpu: &TextureContext<'_>, atlas: &Arc<SymbolAtlas>) -> Self {
        let texture = gpu.device.create_texture_with_data(
            gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("symbol atlas"),
                size: wgpu::Extent3d {
                    width: atlas.size[0],
                    height: atlas.size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            &atlas.pixels,
        );
        Self {
            source: atlas.clone(),
            texture,
        }
    }
}

impl DrawBinding {
    fn new(gpu: &TextureContext<'_>, atlas: Arc<TileAtlas>, uniforms: SymbolUniforms) -> Self {
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("symbol paint"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let view = atlas.texture.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("symbol atlas and paint"),
            layout: &gpu.pipeline.get_bind_group_layout(1),
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
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        Self {
            atlas,
            group,
            buffer,
            uniforms,
        }
    }
}

//! GPU atlases shared by layers and their evaluated draw uniforms.
use std::sync::{Arc, Weak};

use wgpu::util::TextureDataOrder;

use super::{assets::SymbolAtlas, paint::SymbolUniforms};
use crate::{coords::WorldTileCoords, render::resource::share_gpu, style::layer::SymbolPaint};

struct TileAtlas {
    source: Arc<SymbolAtlas>,
    texture: wgpu::Texture,
}

impl Drop for TileAtlas {
    fn drop(&mut self) {
        self.texture.destroy();
    }
}

pub(crate) struct DrawBinding {
    atlas: Arc<TileAtlas>,
    pub group: wgpu::BindGroup,
    pub separate_halo: bool,
    buffer: wgpu::Buffer,
    uniforms: SymbolUniforms,
}

impl Drop for DrawBinding {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub(super) struct TextureContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a crate::render::upload_queue::UploadQueue,
    pub atlas_layout: &'a wgpu::BindGroupLayout,
}

#[derive(Default)]
pub(crate) struct SymbolTextures {
    atlases: rustc_hash::FxHashMap<usize, Weak<TileAtlas>>,
    tile_atlases: rustc_hash::FxHashMap<(WorldTileCoords, usize), Arc<TileAtlas>>,
    bindings: rustc_hash::FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), DrawBinding>,
    combined_count: usize,
    separate_count: usize,
    sampler: Option<wgpu::Sampler>,
    /// Tiles whose labels gained an atlas since the last drain.
    bound: Vec<WorldTileCoords>,
    last_retain_key: Option<(u64, usize, u64)>,
}

impl SymbolTextures {
    pub(super) fn retain_if_needed(
        &mut self,
        tiles: &crate::tcs::tiles::Tiles,
        style: &crate::style::Style,
        eviction_epoch: u64,
    ) {
        let key = (style.revision(), tiles.tiles.len(), eviction_epoch);
        if self.last_retain_key == Some(key) {
            return;
        }
        self.last_retain_key = Some(key);
        self.retain(tiles, style);
    }
    pub(super) fn prepare(
        &mut self,
        gpu: &TextureContext<'_>,
        key: (WorldTileCoords, crate::style::StyleLayerId),
        atlas: &Arc<SymbolAtlas>,
        paint: &SymbolPaint,
        zoom: f64,
    ) {
        let uniforms = SymbolUniforms::new(paint, zoom, atlas.size);
        let separate_halo = uniforms.text[1] > 1.0
            || uniforms.icon[1] > 1.0
            || (uniforms.text[1] > 0.0
                && paint.number("text-letter-spacing", &Default::default(), zoom, 0.0) < 0.0);
        if let Some(binding) = self.bindings.get_mut(&key) {
            if binding.atlas.source.same_gpu_atlas(atlas) {
                if binding.separate_halo != separate_halo {
                    if binding.separate_halo {
                        self.separate_count = self.separate_count.saturating_sub(1);
                        self.combined_count = self.combined_count.saturating_add(1);
                    } else {
                        self.combined_count = self.combined_count.saturating_sub(1);
                        self.separate_count = self.separate_count.saturating_add(1);
                    }
                    binding.separate_halo = separate_halo;
                }
                if binding.uniforms != uniforms {
                    gpu.queue
                        .write_buffer(&binding.buffer, 0, bytemuck::bytes_of(&uniforms));
                    binding.uniforms = uniforms;
                }
                return;
            }
        }
        // Source atlases can share tile coordinates without sharing sprite or font pixels.
        let identity = atlas.gpu_identity();
        let texture = self
            .tile_atlases
            .get(&(key.0, identity))
            .filter(|cached| cached.source.same_gpu_atlas(atlas))
            .cloned()
            .or_else(|| {
                self.atlases
                    .get(&identity)
                    .and_then(Weak::upgrade)
                    .filter(|cached| cached.source.same_gpu_atlas(atlas))
            })
            .unwrap_or_else(|| {
                let texture = share_gpu(TileAtlas::new(gpu, atlas));
                self.atlases.insert(identity, Arc::downgrade(&texture));
                texture
            });
        self.tile_atlases
            .insert((key.0, identity), Arc::clone(&texture));
        if !self.bound.contains(&key.0) {
            self.bound.push(key.0);
        }
        let sampler = self.sampler.get_or_insert_with(|| {
            gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        });
        let binding = DrawBinding::new(gpu, sampler, texture, uniforms, separate_halo);
        if separate_halo {
            self.separate_count = self.separate_count.saturating_add(1);
        } else {
            self.combined_count = self.combined_count.saturating_add(1);
        }
        if let Some(old) = self.bindings.insert(key, binding) {
            if old.separate_halo {
                self.separate_count = self.separate_count.saturating_sub(1);
            } else {
                self.combined_count = self.combined_count.saturating_sub(1);
            }
            gpu.queue.cancel_staged_buffer(&old.buffer);
        }
    }

    /// Tiles whose glyphs and icons became drawable since the last call.
    pub fn take_bound(&mut self) -> Vec<WorldTileCoords> {
        std::mem::take(&mut self.bound)
    }

    /// Returns `(any_combined, any_separate_halo)` across currently prepared draw bindings.
    pub fn separate_halo_flags(&self) -> (bool, bool) {
        (self.combined_count > 0, self.separate_count > 0)
    }

    #[cfg(all(test, feature = "headless"))]
    pub fn binding(&self, coords: WorldTileCoords, layer: &str) -> Option<&DrawBinding> {
        self.binding_by_id(coords, crate::style::StyleLayerId::intern(layer))
    }

    #[inline]
    pub(crate) fn binding_by_id(
        &self,
        coords: WorldTileCoords,
        layer: crate::style::StyleLayerId,
    ) -> Option<&DrawBinding> {
        self.bindings.get(&(coords, layer))
    }

    pub fn clear(&mut self) {
        self.atlases.clear();
        self.tile_atlases.clear();
        self.bindings.clear();
        self.combined_count = 0;
        self.separate_count = 0;
        self.bound.clear();
        self.sampler = None;
        self.last_retain_key = None;
    }

    pub fn retain(&mut self, tiles: &crate::tcs::tiles::Tiles, style: &crate::style::Style) {
        let symbol_layers: rustc_hash::FxHashSet<crate::style::StyleLayerId> = style
            .layers
            .iter()
            .filter(|layer| layer.type_ == "symbol")
            .map(|layer| crate::style::StyleLayerId::intern(&layer.id))
            .collect();
        if symbol_layers.is_empty() {
            self.clear();
            return;
        }
        self.bindings.retain(|(coords, layer), _| {
            symbol_layers.contains(layer)
                && tiles
                    .query::<&super::SymbolLayersDataComponent>(*coords)
                    .is_some()
        });
        if self.bindings.is_empty() {
            self.clear();
            return;
        }
        let mut combined_count = 0_usize;
        let mut separate_count = 0_usize;
        for binding in self.bindings.values() {
            if binding.separate_halo {
                separate_count += 1;
            } else {
                combined_count += 1;
            }
        }
        self.combined_count = combined_count;
        self.separate_count = separate_count;
        let referenced: rustc_hash::FxHashSet<(WorldTileCoords, usize)> = self
            .bindings
            .iter()
            .map(|((coords, _), binding)| (*coords, Arc::as_ptr(&binding.atlas) as usize))
            .collect();
        self.tile_atlases.retain(|&(coords, _), atlas| {
            referenced.contains(&(coords, Arc::as_ptr(atlas) as usize))
        });
        self.atlases.retain(|_, atlas| atlas.strong_count() > 0);
    }
}

impl TileAtlas {
    fn new(gpu: &TextureContext<'_>, atlas: &Arc<SymbolAtlas>) -> Self {
        let pixel_bytes = atlas.take_pixels_for_gpu();
        let expected_len = (atlas.size[0] as usize)
            .saturating_mul(atlas.size[1] as usize)
            .saturating_mul(4);
        let fallback_zeroes;
        let upload_slice: &[u8] = if pixel_bytes.len() == expected_len {
            &pixel_bytes
        } else {
            fallback_zeroes = vec![0_u8; expected_len];
            &fallback_zeroes
        };
        let texture = gpu.queue.create_texture_with_data(
            gpu.device,
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
            upload_slice,
        );
        Self {
            source: atlas.clone(),
            texture,
        }
    }
}

impl DrawBinding {
    fn new(
        gpu: &TextureContext<'_>,
        sampler: &wgpu::Sampler,
        atlas: Arc<TileAtlas>,
        uniforms: SymbolUniforms,
        separate_halo: bool,
    ) -> Self {
        let buffer = gpu.queue.create_buffer_init(
            gpu.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("symbol paint"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let view = atlas.texture.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("symbol atlas and paint"),
            layout: gpu.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
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
            separate_halo,
            buffer,
            uniforms,
        }
    }
}

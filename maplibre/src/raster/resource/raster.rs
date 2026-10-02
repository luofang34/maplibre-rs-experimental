use std::collections::{HashMap, HashSet};

use crate::{
    coords::WorldTileCoords,
    raster::{paint::RasterUniforms, RasterSourceId},
    render::{
        resource::{MipmapGenerator, Texture},
        settings::Msaa,
        tile_view_pattern::HasTile,
    },
    style::Style,
    tcs::world::World,
};

/// Raster pipeline and sampler with independent bindings for each source and tile.
/// Raster imagery and DEM shading share storage without sharing source identities.
pub struct RasterResources {
    sampler: wgpu::Sampler,
    mipmaps: MipmapGenerator,
    msaa: Msaa,
    pipeline: wgpu::RenderPipeline,
    bound_textures: HashMap<RasterSourceId, HashMap<WorldTileCoords, (wgpu::BindGroup, u64)>>,
    layer_sources: HashMap<String, RasterSourceId>,
    /// Sources whose textures carry a border of neighbour samples for slope shading.
    dem_sources: HashSet<RasterSourceId>,
    /// For each bordered texture, the sides whose border holds a real neighbour's samples.
    border_sides: HashMap<RasterSourceId, HashMap<WorldTileCoords, u16>>,
    /// Uniform buffer and bind group of each raster layer's paint adjustments, and of the
    /// same paint for the layer's departing tiles.
    layer_paints: HashMap<(String, bool), (wgpu::Buffer, wgpu::BindGroup)>,
    /// Advances whenever a texture is bound, so cached renders of raster tiles can refresh.
    revision: u64,
}

impl RasterResources {
    /// Creates an empty texture registry with linear filtering.
    /// `msaa` controls textures allocated by `create_texture`, not the supplied pipeline.
    pub fn new(msaa: Msaa, device: &wgpu::Device, pipeline: wgpu::RenderPipeline) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // GL JS picks the nearest level for raster tiles rather than blending two.
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            sampler,
            mipmaps: MipmapGenerator::new(device, wgpu::TextureFormat::Rgba8Unorm),
            msaa,
            pipeline,
            bound_textures: Default::default(),
            layer_sources: Default::default(),
            dem_sources: Default::default(),
            border_sides: Default::default(),
            layer_paints: Default::default(),
            revision: 0,
        }
    }

    /// Wrapping generation advanced by each binding insertion or replacement; removal leaves it unchanged.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Allocates an uninitialized texture with this registry's sample count.
    /// Dimensions are texels; invalid format, usage or dimension combinations fail wgpu validation.
    pub fn create_texture(
        &mut self,
        label: wgpu::Label,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> Texture {
        Texture::new(label, device, format, width, height, self.msaa, usage)
    }

    /// Allocates an imagery texture with a full mip chain, so tiles drawn smaller than their
    /// pixels are minified as GL JS does. Fill level zero, then call [`Self::generate_mipmaps`].
    pub fn create_imagery_texture(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Texture {
        Texture::new_mipmapped(
            None,
            device,
            wgpu::TextureFormat::Rgba8Unorm,
            width,
            height,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
    }

    /// Fills the levels below the first from the level above.
    pub fn generate_mipmaps(
        &self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        texture: &Texture,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.mipmaps
            .generate(device, &mut encoder, &texture.texture);
        queue.submit([encoder.finish()]);
    }

    /// Borrows the current texture/sampler binding, or `None` before upload or after eviction.
    pub fn get_bound_texture(
        &self,
        source: &RasterSourceId,
        coords: &WorldTileCoords,
    ) -> Option<&wgpu::BindGroup> {
        self.bound_textures
            .get(source)?
            .get(coords)
            .map(|(binding, _)| binding)
    }

    pub(crate) fn texture_revision(&self, layer: &str, coords: WorldTileCoords) -> Option<u64> {
        self.bound_textures
            .get(self.layer_sources.get(layer)?)?
            .get(&coords)
            .map(|(_, revision)| *revision)
    }

    /// Drops every source binding at these coordinates without advancing the insertion generation.
    pub fn remove_texture(&mut self, coords: WorldTileCoords) {
        for textures in self.bound_textures.values_mut() {
            textures.remove(&coords);
        }
        for sides in self.border_sides.values_mut() {
            sides.remove(&coords);
        }
    }

    /// Inserts or replaces the texture/sampler binding at group one of the raster pipeline.
    /// The bind group retains the texture view after the supplied wrapper is dropped.
    pub fn bind_texture(
        &mut self,
        device: &wgpu::Device,
        source: &RasterSourceId,
        coords: &WorldTileCoords,
        texture: Texture,
    ) {
        self.revision = self.revision.wrapping_add(1);
        self.bound_textures
            .entry(source.clone())
            .or_default()
            .insert(
                *coords,
                (
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &self.pipeline.get_bind_group_layout(1),
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&texture.view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                        label: None,
                    }),
                    self.revision,
                ),
            );
    }

    /// Drops every texture of a source, as when it leaves the style.
    pub(crate) fn forget_source(&mut self, source: &RasterSourceId) {
        self.bound_textures.remove(source);
        self.border_sides.remove(source);
    }

    /// Whether a layer of the style draws from the source, so its tiles need textures.
    pub(crate) fn draws(&self, source: &RasterSourceId) -> bool {
        self.dem_sources.contains(source)
            || self.layer_sources.values().any(|drawn| drawn == source)
    }

    /// The number of tile textures bound for a source.
    #[cfg(test)]
    pub(crate) fn texture_count(&self, source: &RasterSourceId) -> usize {
        self.bound_textures.get(source).map_or(0, HashMap::len)
    }

    /// Drops one source binding while retaining other sources at these coordinates.
    pub fn remove_source_texture(&mut self, source: &RasterSourceId, coords: WorldTileCoords) {
        if let Some(textures) = self.bound_textures.get_mut(source) {
            textures.remove(&coords);
        }
        if let Some(sides) = self.border_sides.get_mut(source) {
            sides.remove(&coords);
        }
    }

    pub(crate) fn update_layer_sources(&mut self, style: &Style) {
        self.layer_sources.clear();
        self.dem_sources = style
            .sources
            .iter()
            .filter(|(_, source)| matches!(source, crate::style::source::Source::RasterDem(_)))
            .map(|(name, _)| RasterSourceId::from(name.as_str()))
            .collect();
        for group in crate::io::tile_sources::source_layer_groups(
            style,
            crate::io::tile_sources::TileKind::Raster,
        ) {
            let source = RasterSourceId::new(group.source_name);
            for layer in group.layers {
                self.layer_sources.insert(layer.id, source.clone());
            }
        }
    }

    /// Records which sides of a freshly uploaded bordered texture hold a neighbour's samples.
    pub(crate) fn set_border_sides(
        &mut self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        sides: u16,
    ) {
        self.border_sides
            .entry(source.clone())
            .or_default()
            .insert(coords, sides);
    }

    /// Drops the texture of a tile whose data changed, and the neighbour textures that carry
    /// its old or replicated edge in their border.
    pub(crate) fn tile_data_changed(&mut self, source: &RasterSourceId, coords: WorldTileCoords) {
        let reloaded = self.get_bound_texture(source, &coords).is_some();
        self.remove_source_texture(source, coords);
        if self.has_border(source) {
            self.refresh_neighbour_borders(source, coords, reloaded);
        }
    }

    /// Drops the bordered textures around a tile whose samples they lack, so they upload again
    /// with the real edge. A tile that already had a texture is reloading, and its neighbours
    /// may hold stale samples, so all of them go.
    pub(crate) fn refresh_neighbour_borders(
        &mut self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        reloaded: bool,
    ) {
        for (neighbour, (dx, dy)) in crate::terrain::backfill::neighbours(coords) {
            let toward_tile = crate::raster::dem_border::side_bit(-dx, -dy);
            let sides = self
                .border_sides
                .get(source)
                .and_then(|sides| sides.get(&neighbour))
                .copied()
                .unwrap_or(0);
            if reloaded || sides & toward_tile == 0 {
                self.remove_source_texture(source, neighbour);
            }
        }
    }

    /// Whether textures of this source are uploaded with a one-texel border.
    pub(crate) fn has_border(&self, source: &RasterSourceId) -> bool {
        self.dem_sources.contains(source)
    }

    pub(crate) fn layer_source(&self, layer: &str) -> Option<&RasterSourceId> {
        self.layer_sources.get(layer)
    }

    pub(crate) fn layer_texture(
        &self,
        layer: &str,
        coords: &WorldTileCoords,
    ) -> Option<&wgpu::BindGroup> {
        self.get_bound_texture(self.layer_source(layer)?, coords)
    }

    /// Writes a layer's paint uniforms, creating its buffer on first use.
    pub fn write_layer_paint(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        layer_id: &str,
        uniforms: &RasterUniforms,
    ) {
        self.write_paint(device, queue, (layer_id, false), uniforms);
    }

    /// Writes the paint the layer's departing tiles draw with while they fade out.
    pub fn write_departing_paint(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        layer_id: &str,
        uniforms: &RasterUniforms,
    ) {
        self.write_paint(device, queue, (layer_id, true), uniforms);
    }

    fn write_paint(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        (layer_id, departing): (&str, bool),
        uniforms: &RasterUniforms,
    ) {
        let key = (layer_id.to_owned(), departing);
        let pipeline = &self.pipeline;
        let (buffer, _) = self.layer_paints.entry(key).or_insert_with(|| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Raster layer paint"),
                size: std::mem::size_of::<RasterUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Raster layer paint"),
                layout: &pipeline.get_bind_group_layout(2),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, bind_group)
        });
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(uniforms));
    }

    /// The paint bind group of a layer written this frame, or of its departing tiles.
    pub(crate) fn layer_paint(&self, layer_id: &str, departing: bool) -> Option<&wgpu::BindGroup> {
        self.layer_paints
            .get(&(layer_id.to_owned(), departing))
            .map(|(_, group)| group)
    }

    /// Borrows the raster pipeline whose group-one layout defines the texture bindings.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

impl HasTile for RasterResources {
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        _world: &World,
    ) -> bool {
        self.get_bound_texture(source, &coords).is_some()
    }

    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.bound_textures
            .values()
            .any(|textures| textures.contains_key(&coords))
    }
}

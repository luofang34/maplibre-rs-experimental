//! Symbol geometry, collision ranges and assets crossing the worker boundary.
use super::*;

impl SymbolLayerTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::SymbolLayerTessellated
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        let estimated_capacity = 1024
            + buffer.buffer.vertices.len() * std::mem::size_of::<FlatSymbolVertex>()
            + buffer.buffer.indices.len() * 4;
        let mut inner_builder = FlatBufferBuilder::with_capacity(estimated_capacity);

        let vertices = inner_builder.create_vector(&flat_vertices(&buffer.buffer.vertices));
        let indices = inner_builder.create_vector(&buffer.buffer.indices);

        let layer_name = inner_builder.create_string(&layer_data.name);
        let style_layer_id_fb = inner_builder.create_string(&style_layer_id);

        let features: Vec<_> = features
            .iter()
            .map(maplibre::sdf::assets::wire::SymbolFeature::from)
            .collect();
        let features = serde_json::to_vec(&features)
            .map_err(|error| tracing::error!(%error, "cannot encode symbol features"))
            .ok();
        let atlas = atlas.map(|atlas| encode_symbol_atlas(&atlas));
        let features = features
            .as_ref()
            .map(|bytes| inner_builder.create_vector(bytes));
        let atlas = atlas
            .as_ref()
            .map(|bytes| inner_builder.create_vector(bytes));
        let mut builder = FlatSymbolLayerTessellatedBuilder::new(&mut inner_builder);
        if let Some(features) = features {
            builder.add_symbol_features(features);
        }
        if let Some(atlas) = atlas {
            builder.add_symbol_atlas(atlas);
        }

        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        builder.add_layer_name(layer_name);
        builder.add_vertices(vertices);
        builder.add_indices(indices);
        builder.add_usable_indices(buffer.usable_indices);
        builder.add_style_layer_id(style_layer_id_fb);
        let root = builder.finish();

        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::SymbolLayerTessellated,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_symbol_layer_tessellated(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }

    fn is_empty(&self) -> bool {
        root_as_flat_symbol_layer_tessellated(&self.data[self.start..])
            .map(|data| data.usable_indices() == 0)
            .unwrap_or(true)
    }

    fn to_bucket(self) -> SymbolLayerData {
        let coords = SymbolLayerTessellated::coords(&self);
        let Ok(data) = root_as_flat_symbol_layer_tessellated(&self.data[self.start..]) else {
            return SymbolLayerData {
                coords,
                source_layer: String::new(),
                style_layer_id: String::new(),
                buffer: OverAlignedVertexBuffer::empty(),
                features: Vec::new(),
                atlas: None,
            };
        };
        let vertices = data
            .vertices()
            .map(|v| {
                v.iter()
                    .map(|vertex| ShaderSymbolVertex {
                        a_pos_offset: vertex.a_pos_offset().into(),
                        a_data: vertex.a_data().into(),
                        a_pixeloffset: vertex.a_pixeloffset().into(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let indices: Vec<u32> = data
            .indices()
            .map(|i| i.iter().collect())
            .unwrap_or_default();
        let usable_indices = data.usable_indices();

        let layer_name = data.layer_name().unwrap_or_default().to_owned();
        let style_layer_id = data
            .style_layer_id()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| layer_name.clone());
        SymbolLayerData {
            coords,
            source_layer: layer_name,
            style_layer_id,
            buffer: OverAlignedVertexBuffer::from_iters(vertices, indices, usable_indices),
            features: data
                .symbol_features()
                .and_then(|bytes| {
                    serde_json::from_slice::<Vec<maplibre::sdf::assets::wire::SymbolFeature>>(
                        bytes.bytes(),
                    )
                    .map_err(|error| tracing::error!(%error, "invalid symbol features"))
                    .ok()
                })
                .unwrap_or_default()
                .into_iter()
                .map(Feature::from)
                .collect(),
            atlas: data
                .symbol_atlas()
                .and_then(|bytes| decode_symbol_atlas(coords, bytes.bytes())),
        }
    }
}

fn encode_symbol_atlas(atlas: &maplibre::sdf::assets::SymbolAtlas) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + atlas.pixels.len());
    out.extend_from_slice(&atlas.size[0].to_le_bytes());
    out.extend_from_slice(&atlas.size[1].to_le_bytes());
    out.extend_from_slice(&atlas.pixels);
    out
}

fn decode_symbol_atlas(
    coords: WorldTileCoords,
    bytes: &[u8],
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    if bytes.len() < 8 {
        return serde_json::from_slice(bytes).ok().map(std::sync::Arc::new);
    }
    let width = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let height = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let pixels = &bytes[8..];
    if pixels.len()
        != (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
    {
        return serde_json::from_slice(bytes).ok().map(std::sync::Arc::new);
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    pixels.hash(&mut hasher);
    let key = (coords, width, height, hasher.finish());
    type AtlasCacheKey = (WorldTileCoords, u32, u32, u64);
    type AtlasCacheMap = std::collections::HashMap<
        AtlasCacheKey,
        std::sync::Weak<maplibre::sdf::assets::SymbolAtlas>,
    >;
    thread_local! {
        static ATLAS_CACHE: std::cell::RefCell<AtlasCacheMap> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    ATLAS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(existing) = cache.get(&key).and_then(std::sync::Weak::upgrade) {
            return Some(existing);
        }
        cache.retain(|_, weak| weak.strong_count() > 0);
        let arc = std::sync::Arc::new(maplibre::sdf::assets::SymbolAtlas {
            size: [width, height],
            pixels: pixels.to_vec(),
            ..Default::default()
        });
        cache.insert(key, std::sync::Arc::downgrade(&arc));
        Some(arc)
    })
}

fn flat_vertices(vertices: &[ShaderSymbolVertex]) -> Vec<FlatSymbolVertex> {
    vertices
        .iter()
        .map(|vertex| {
            FlatSymbolVertex::new(&vertex.a_pos_offset, &vertex.a_data, &vertex.a_pixeloffset)
        })
        .collect()
}

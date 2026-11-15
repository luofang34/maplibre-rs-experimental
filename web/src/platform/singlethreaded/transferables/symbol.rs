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

        let usable_indices = buffer.usable_indices;
        let raw_vertices = buffer.buffer.vertices;
        let raw_indices = buffer.buffer.indices;
        let flat_verts = flat_vertices(&raw_vertices);
        drop(raw_vertices);
        let vertices = inner_builder.create_vector(&flat_verts);
        drop(flat_verts);
        let indices = inner_builder.create_vector(&raw_indices);
        drop(raw_indices);

        let layer_name = inner_builder.create_string(&layer_data.name);
        let style_layer_id_fb = inner_builder.create_string(&style_layer_id);

        let feature_bytes = maplibre::sdf::assets::wire::encode_symbol_features(&features);
        drop(features);
        let atlas = atlas.map(|atlas| encode_symbol_atlas(coords, &atlas));
        let features = Some(inner_builder.create_vector(&feature_bytes));
        drop(feature_bytes);
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
        builder.add_usable_indices(usable_indices);
        builder.add_style_layer_id(style_layer_id_fb);
        let root = builder.finish();

        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable::from_vec(WebMessageTag::SymbolLayerTessellated, data, start)
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
        let (vertices, indices, usable_indices, layer_name, style_layer_id, features, atlas_span) = {
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
            let vertices = if let Some(v) = data.vertices() {
                let mut out = Vec::with_capacity(v.len());
                for vertex in v.iter() {
                    out.push(ShaderSymbolVertex {
                        a_pos_offset: vertex.a_pos_offset().into(),
                        a_data: vertex.a_data().into(),
                        a_pixel_and_height: vertex.a_pixel_and_height().into(),
                    });
                }
                out
            } else {
                Vec::new()
            };

            let indices = if let Some(i) = data.indices() {
                let mut out = Vec::with_capacity(i.len());
                out.extend(i.iter());
                out
            } else {
                Vec::new()
            };
            let usable_indices = data.usable_indices();

            let layer_name = data.layer_name().unwrap_or_default().to_owned();
            let style_layer_id = data
                .style_layer_id()
                .map(|s| s.to_owned())
                .unwrap_or_else(|| layer_name.clone());
            let features = data
                .symbol_features()
                .and_then(|bytes| {
                    let decoded =
                        maplibre::sdf::assets::wire::decode_symbol_features(bytes.bytes());
                    if decoded.is_none() {
                        tracing::error!("invalid symbol features");
                    }
                    decoded
                })
                .unwrap_or_default();
            let base_addr = self.data.as_ptr() as usize;
            let atlas_span = data.symbol_atlas().map(|vec| {
                let slice = vec.bytes();
                let offset = (slice.as_ptr() as usize).saturating_sub(base_addr);
                (offset, slice.len())
            });
            (
                vertices,
                indices,
                usable_indices,
                layer_name,
                style_layer_id,
                features,
                atlas_span,
            )
        };
        let atlas = atlas_span
            .and_then(|(offset, len)| decode_symbol_atlas_owned(coords, self.data, offset, len));
        SymbolLayerData {
            coords,
            source_layer: layer_name,
            style_layer_id,
            buffer: OverAlignedVertexBuffer::from_vecs(vertices, indices, usable_indices),
            features,
            atlas,
        }
    }
}

const ATLAS_WIRE_MAGIC: u32 = u32::MAX;
const ATLAS_WIRE_FULL: u32 = 1;
const ATLAS_WIRE_REF: u32 = 2;
const RECENT_ATLAS_RING_CAP: usize = 1;

fn encode_symbol_atlas(
    coords: WorldTileCoords,
    atlas: &std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>,
) -> Vec<u8> {
    type SentKey = (WorldTileCoords, usize);
    type SentMap =
        std::collections::HashMap<SentKey, std::sync::Weak<maplibre::sdf::assets::SymbolAtlas>>;
    thread_local! {
        static SENT_ATLASES: std::cell::RefCell<SentMap> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let ptr_usize = std::sync::Arc::as_ptr(atlas) as usize;
    let ptr_id = ptr_usize as u64;
    let already_sent = SENT_ATLASES.with(|sent| {
        let mut sent = sent.borrow_mut();
        if let Some(existing) = sent
            .get(&(coords, ptr_usize))
            .and_then(std::sync::Weak::upgrade)
        {
            if std::sync::Arc::ptr_eq(&existing, atlas) {
                return true;
            }
        }
        if sent.len() >= 64 {
            sent.retain(|_, weak| weak.strong_count() > 0);
        }
        sent.insert((coords, ptr_usize), std::sync::Arc::downgrade(atlas));
        false
    });
    if already_sent {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&ATLAS_WIRE_MAGIC.to_le_bytes());
        out.extend_from_slice(&ATLAS_WIRE_REF.to_le_bytes());
        out.extend_from_slice(&atlas.size[0].to_le_bytes());
        out.extend_from_slice(&atlas.size[1].to_le_bytes());
        out.extend_from_slice(&ptr_id.to_le_bytes());
        return out;
    }
    atlas.with_pixels(|pixels| {
        let mut out = Vec::with_capacity(24 + pixels.len());
        out.extend_from_slice(&ATLAS_WIRE_MAGIC.to_le_bytes());
        out.extend_from_slice(&ATLAS_WIRE_FULL.to_le_bytes());
        out.extend_from_slice(&atlas.size[0].to_le_bytes());
        out.extend_from_slice(&atlas.size[1].to_le_bytes());
        out.extend_from_slice(&ptr_id.to_le_bytes());
        out.extend_from_slice(pixels);
        out
    })
}

fn decode_symbol_atlas_owned(
    coords: WorldTileCoords,
    mut owned_data: Vec<u8>,
    offset: usize,
    len: usize,
) -> Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>> {
    let end = offset.checked_add(len)?;
    let bytes = owned_data.get(offset..end)?;
    if bytes.len() < 8 {
        return serde_json::from_slice(bytes).ok().map(std::sync::Arc::new);
    }
    type AtlasCacheKey = (WorldTileCoords, u32, u32, u64);
    struct MainAtlasCache {
        weak: std::collections::HashMap<
            AtlasCacheKey,
            std::sync::Weak<maplibre::sdf::assets::SymbolAtlas>,
        >,
        recent: std::collections::VecDeque<(
            AtlasCacheKey,
            std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>,
        )>,
    }
    thread_local! {
        static ATLAS_CACHE: std::cell::RefCell<MainAtlasCache> =
            std::cell::RefCell::new(MainAtlasCache {
                weak: std::collections::HashMap::new(),
                recent: std::collections::VecDeque::with_capacity(RECENT_ATLAS_RING_CAP),
            });
    }
    let first_word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if first_word == ATLAS_WIRE_MAGIC && bytes.len() >= 24 {
        let kind = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let width = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let height = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let ptr_id = u64::from_le_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]);
        let key = (coords, width, height, ptr_id);
        if let Some(hit) = ATLAS_CACHE.with(|cache| {
            let cache = cache.borrow();
            if let Some((_, arc)) = cache.recent.iter().find(|(k, _)| *k == key) {
                return Some(std::sync::Arc::clone(arc));
            }
            cache.weak.get(&key).and_then(std::sync::Weak::upgrade)
        }) {
            return Some(hit);
        }
        if kind == ATLAS_WIRE_REF {
            return None;
        }
        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if len.saturating_sub(24) != expected_len {
            return None;
        }
        owned_data.truncate(end);
        owned_data.drain(..offset + 24);
        owned_data.shrink_to_fit();
        return ATLAS_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.weak.retain(|_, weak| weak.strong_count() > 0);
            let arc = std::sync::Arc::new(maplibre::sdf::assets::SymbolAtlas::with_wasm_staging(
                [width, height],
                owned_data,
            ));
            cache.weak.insert(key, std::sync::Arc::downgrade(&arc));
            if cache.recent.len() >= RECENT_ATLAS_RING_CAP {
                cache.recent.pop_front();
            }
            cache.recent.push_back((key, std::sync::Arc::clone(&arc)));
            Some(arc)
        });
    }
    let width = first_word;
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
    if let Some(existing) = ATLAS_CACHE.with(|cache| {
        cache
            .borrow()
            .weak
            .get(&key)
            .and_then(std::sync::Weak::upgrade)
    }) {
        return Some(existing);
    }
    owned_data.truncate(end);
    owned_data.drain(..offset + 8);
    owned_data.shrink_to_fit();
    ATLAS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.weak.retain(|_, weak| weak.strong_count() > 0);
        let arc = std::sync::Arc::new(maplibre::sdf::assets::SymbolAtlas::with_wasm_staging(
            [width, height],
            owned_data,
        ));
        cache.weak.insert(key, std::sync::Arc::downgrade(&arc));
        Some(arc)
    })
}

fn flat_vertices(vertices: &[ShaderSymbolVertex]) -> Vec<FlatSymbolVertex> {
    vertices
        .iter()
        .map(|vertex| {
            FlatSymbolVertex::new(
                &vertex.a_pos_offset,
                &vertex.a_data,
                &vertex.a_pixel_and_height,
            )
        })
        .collect()
}

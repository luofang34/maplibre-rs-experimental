//! Symbol geometry, collision ranges and assets crossing the worker boundary.
use super::*;

impl SymbolLayerTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::SymbolLayerTessellated
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
        new_buffer: OverAlignedVertexBuffer<ShaderSymbolVertexNew, IndexDataType>,
        features: Vec<Feature>,
        atlas: Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        let mut inner_builder = FlatBufferBuilder::with_capacity(1024);

        let vertices = inner_builder.create_vector(&flat_vertices(&buffer.buffer.vertices));
        let indices = inner_builder.create_vector(&buffer.buffer.indices);

        let new_vertices =
            inner_builder.create_vector(&flat_new_vertices(&new_buffer.buffer.vertices));
        let new_indices = inner_builder.create_vector(&new_buffer.buffer.indices);

        let layer_name = inner_builder.create_string(&layer_data.name);
        let style_layer_id_fb = inner_builder.create_string(&style_layer_id);

        let features: Vec<_> = features
            .iter()
            .map(maplibre::sdf::assets::wire::SymbolFeature::from)
            .collect();
        let features = serde_json::to_vec(&features)
            .map_err(|error| tracing::error!(%error, "cannot encode symbol features"))
            .ok();
        let atlas = atlas.and_then(|atlas| {
            serde_json::to_vec(&*atlas)
                .map_err(|error| tracing::error!(%error, "cannot encode symbol atlas"))
                .ok()
        });
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
        builder.add_new_vertices(new_vertices);
        builder.add_new_indices(new_indices);
        builder.add_new_usable_indices(new_buffer.usable_indices);
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
        let data = root_as_flat_symbol_layer_tessellated(&self.data[self.start..]).unwrap();
        data.coords().unwrap().into()
    }

    fn is_empty(&self) -> bool {
        let data = root_as_flat_symbol_layer_tessellated(&self.data[self.start..]).unwrap();
        data.new_usable_indices() == 0
    }

    fn to_bucket(self) -> SymbolLayerData {
        let data = root_as_flat_symbol_layer_tessellated(&self.data[self.start..]).unwrap();
        let vertices = data
            .vertices()
            .unwrap()
            .iter()
            .map(|vertex| ShaderSymbolVertex {
                position: vertex.position().into(),
                text_anchor: vertex.text_anchor().into(),
                tex_coords: vertex.tex_coords().into(),
                color: vertex.color().into(),
                is_glyph: vertex.is_glyph(),
            });

        let indices = data.indices().unwrap();
        let usable_indices = data.usable_indices();

        let new_vertices = data
            .new_vertices()
            .map(|v| {
                v.iter()
                    .map(|vertex| ShaderSymbolVertexNew {
                        a_pos_offset: vertex.a_pos_offset().into(),
                        a_data: vertex.a_data().into(),
                        a_pixeloffset: vertex.a_pixeloffset().into(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let new_indices: Vec<u32> = data
            .new_indices()
            .map(|i| i.iter().collect())
            .unwrap_or_default();
        let new_usable_indices = data.new_usable_indices();

        let layer_name = data.layer_name().unwrap().to_owned();
        let style_layer_id = data
            .style_layer_id()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| layer_name.clone());
        SymbolLayerData {
            coords: SymbolLayerTessellated::coords(&self),
            source_layer: layer_name,
            style_layer_id,
            buffer: OverAlignedVertexBuffer::from_iters(vertices, indices, usable_indices),
            new_buffer: OverAlignedVertexBuffer::from_iters(
                new_vertices.into_iter(),
                new_indices.into_iter(),
                new_usable_indices,
            ),
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
                .and_then(|bytes| {
                    serde_json::from_slice(bytes.bytes())
                        .map_err(|error| tracing::error!(%error, "invalid symbol atlas"))
                        .ok()
                })
                .map(std::sync::Arc::new),
        }
    }
}

fn flat_vertices(vertices: &[ShaderSymbolVertex]) -> Vec<FlatSymbolVertex> {
    vertices
        .iter()
        .map(|vertex| {
            FlatSymbolVertex::new(
                &vertex.position,
                &vertex.text_anchor,
                &vertex.tex_coords,
                &vertex.color,
                vertex.is_glyph,
            )
        })
        .collect()
}
fn flat_new_vertices(vertices: &[ShaderSymbolVertexNew]) -> Vec<FlatSymbolVertexNew> {
    vertices
        .iter()
        .map(|vertex| {
            FlatSymbolVertexNew::new(&vertex.a_pos_offset, &vertex.a_data, &vertex.a_pixeloffset)
        })
        .collect()
}

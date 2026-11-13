use flatbuffers::FlatBufferBuilder;
use maplibre::{
    benchmarking::tessellation::{IndexDataType, OverAlignedVertexBuffer},
    coords::{WorldTileCoords, ZoomLevel},
    io::apc::MessageTag,
    render::ShaderVertex,
    tile::Layer,
    vector::{
        feature_paint::{
            wire::{decode_feature_paint, encode_feature_paint},
            BucketFeatureEntry,
        },
        AvailableVectorLayerBucket, LayerTessellated,
    },
};

use super::{
    basic_generated::FlatWorldTileCoords,
    layer_tessellated_generated::{
        root_as_flat_layer_tessellated, FlatLayerTessellatedBuilder, FlatShaderVertex,
    },
    FlatBufferTransferable,
};
use crate::platform::singlethreaded::apc::WebMessageTag;

#[allow(clippy::too_many_arguments)]
fn pack_flat_layer_tessellated(
    coords: WorldTileCoords,
    buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    feature_indices: Vec<u32>,
    feature_colors: Vec<[f32; 4]>,
    feature_sort_keys: Vec<f32>,
    layer_name_str: &str,
    style_layer_id: &str,
    paint_bytes: Option<&[u8]>,
) -> FlatBufferTransferable {
    let estimated_capacity = 256
        + buffer.buffer.vertices.len() * 32
        + buffer.buffer.indices.len() * 4
        + feature_indices.len() * 4
        + feature_colors.len() * 16
        + feature_sort_keys.len() * 4
        + paint_bytes.map_or(0, <[u8]>::len);
    let mut inner_builder = FlatBufferBuilder::with_capacity(estimated_capacity);

    let vertices = inner_builder.create_vector(
        &buffer
            .buffer
            .vertices
            .iter()
            .map(|vertex| {
                FlatShaderVertex::new(
                    &vertex.position,
                    &vertex.normal,
                    vertex.elevation,
                    vertex.edge_distance,
                    vertex.corner_code,
                )
            })
            .collect::<Vec<_>>(),
    );
    let indices = inner_builder.create_vector(&buffer.buffer.indices);
    let feature_indices_fb = inner_builder.create_vector(&feature_indices);
    let layer_name = inner_builder.create_string(layer_name_str);
    let style_layer_id_fb = inner_builder.create_string(style_layer_id);
    let flat_colors: Vec<f32> = feature_colors
        .iter()
        .flat_map(|c| c.iter().copied())
        .collect();
    let feature_colors_fb = inner_builder.create_vector(&flat_colors);
    let feature_sort_keys_fb = inner_builder.create_vector(&feature_sort_keys);
    let distances = inner_builder.create_vector(
        &buffer
            .buffer
            .vertices
            .iter()
            .map(|v| v.distance)
            .collect::<Vec<_>>(),
    );
    let feature_paint_fb = paint_bytes.map(|bytes| inner_builder.create_vector(bytes));

    let mut builder = FlatLayerTessellatedBuilder::new(&mut inner_builder);
    builder.add_coords(&FlatWorldTileCoords::new(
        coords.x,
        coords.y,
        coords.z.into(),
    ));
    builder.add_layer_name(layer_name);
    builder.add_vertices(vertices);
    builder.add_indices(indices);
    builder.add_feature_indices(feature_indices_fb);
    builder.add_usable_indices(buffer.usable_indices);
    builder.add_style_layer_id(style_layer_id_fb);
    builder.add_feature_colors(feature_colors_fb);
    builder.add_vertex_distances(distances);
    builder.add_feature_sort_keys(feature_sort_keys_fb);
    if let Some(paint) = feature_paint_fb {
        builder.add_feature_paint(paint);
    }
    let root = builder.finish();

    inner_builder.finish(root, None);
    let (data, start) = inner_builder.collapse();
    FlatBufferTransferable::from_vec(WebMessageTag::LayerTessellated, data, start)
}

impl LayerTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerTessellated
    }

    fn style_layer_id(&self) -> &str {
        root_as_flat_layer_tessellated(self.data())
            .ok()
            .and_then(|data| data.style_layer_id().or_else(|| data.layer_name()))
            .unwrap_or("")
    }

    fn build_from(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
    ) -> Self {
        pack_flat_layer_tessellated(
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            &layer_data.name,
            &style_layer_id,
            None,
        )
    }

    fn build_from_with_paint(
        coords: WorldTileCoords,
        buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
        feature_indices: Vec<u32>,
        feature_colors: Vec<[f32; 4]>,
        feature_sort_keys: Vec<f32>,
        layer_data: Layer,
        style_layer_id: String,
        entries: Vec<BucketFeatureEntry>,
    ) -> Self {
        let encoded = encode_feature_paint(&entries);
        pack_flat_layer_tessellated(
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            &layer_data.name,
            &style_layer_id,
            Some(&encoded),
        )
    }

    fn with_feature_paint(self, entries: Vec<BucketFeatureEntry>) -> Self {
        let encoded = encode_feature_paint(&entries);
        let bucket = self.to_bucket();
        pack_flat_layer_tessellated(
            bucket.coords,
            bucket.buffer,
            bucket.feature_indices,
            bucket.feature_colors,
            bucket.feature_sort_keys,
            &bucket.source_layer,
            &bucket.style_layer_id,
            Some(&encoded),
        )
    }

    fn take_feature_paint(&mut self) -> Option<Vec<BucketFeatureEntry>> {
        let data = root_as_flat_layer_tessellated(self.data()).ok()?;
        let bytes = data.feature_paint()?.bytes();
        decode_feature_paint(bytes)
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_tessellated(self.data())
            .ok()
            .and_then(|data| data.coords())
            .map(Into::into)
            .unwrap_or(WorldTileCoords {
                x: 0,
                y: 0,
                z: ZoomLevel::new(0),
            })
    }

    fn is_empty(&self) -> bool {
        root_as_flat_layer_tessellated(self.data())
            .map(|data| data.usable_indices() == 0)
            .unwrap_or(true)
    }

    fn to_bucket(self) -> AvailableVectorLayerBucket {
        let coords = LayerTessellated::coords(&self);
        let Ok(data) = root_as_flat_layer_tessellated(self.data()) else {
            return AvailableVectorLayerBucket {
                coords,
                source_layer: String::new(),
                style_layer_id: String::new(),
                buffer: OverAlignedVertexBuffer::empty(),
                feature_indices: Vec::new(),
                feature_colors: Vec::new(),
                feature_sort_keys: Vec::new(),
            };
        };
        let vertices: Vec<ShaderVertex> = data
            .vertices()
            .into_iter()
            .flat_map(|v| v.iter())
            .enumerate()
            .map(|(index, vertex)| {
                let mut result =
                    ShaderVertex::new(vertex.position().into(), vertex.normal().into());
                result.distance = data
                    .vertex_distances()
                    .and_then(|values| (index < values.len()).then(|| values.get(index)))
                    .unwrap_or(0.0);
                result.elevation = vertex.elevation();
                result.edge_distance = vertex.edge_distance();
                result.corner_code = vertex.corner_code();
                result
            })
            .collect();
        let indices: Vec<u32> = data
            .indices()
            .map(|idx| idx.iter().collect())
            .unwrap_or_default();
        let feature_indices: Vec<u32> = data
            .feature_indices()
            .map(|fi| fi.iter().collect())
            .unwrap_or_default();
        let usable_indices = data.usable_indices();
        let layer_name = data.layer_name().unwrap_or_default().to_owned();
        let style_layer_id = data
            .style_layer_id()
            .map(str::to_owned)
            .unwrap_or_else(|| layer_name.clone());
        let feature_colors: Vec<[f32; 4]> = data
            .feature_colors()
            .map(|fc| {
                let count = fc.len() / 4;
                (0..count)
                    .map(|i| {
                        let base = i * 4;
                        [
                            fc.get(base),
                            fc.get(base + 1),
                            fc.get(base + 2),
                            fc.get(base + 3),
                        ]
                    })
                    .collect()
            })
            .unwrap_or_default();
        AvailableVectorLayerBucket {
            coords,
            source_layer: layer_name,
            style_layer_id,
            buffer: OverAlignedVertexBuffer::from_iters(vertices, indices, usable_indices),
            feature_indices,
            feature_colors,
            feature_sort_keys: data
                .feature_sort_keys()
                .map(|keys| keys.iter().collect())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use maplibre::{
        coords::{WorldTileCoords, ZoomLevel},
        render::ShaderVertex,
        style::expression::{FeatureProperties, Value},
        tile::Layer,
        vector::{
            feature_paint::{BucketEntryKind, BucketFeatureEntry},
            tessellation::OverAlignedVertexBuffer,
            LayerTessellated,
        },
    };
    use wasm_bindgen_test::*;

    use super::FlatBufferTransferable;

    #[wasm_bindgen_test]
    fn layer_tessellated_preserves_all_shader_vertex_fields_and_feature_paint() {
        let packed_elevation = f32::from_bits(0xFF00_1234);
        let input = ShaderVertex {
            position: [128.5, 2048.25],
            normal: [-0.5, 0.75],
            distance: 42.0,
            elevation: packed_elevation,
            edge_distance: 19.5,
            corner_code: 17.0,
        };
        let buffer = OverAlignedVertexBuffer::from_iters([input, input, input], [0_u32, 1, 2], 3);
        let layer = Layer {
            version: 2,
            name: "boundary".into(),
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
            extent: Some(4096),
        };
        let mut props = FeatureProperties::new();
        props.insert("admin_level".into(), Value::Number(2.0));
        let entry = BucketFeatureEntry {
            raw_id: Some(7),
            properties: Arc::new(props.clone()),
            kind: BucketEntryKind::Feature,
        };
        let mut transferable = <FlatBufferTransferable as LayerTessellated>::build_from_with_paint(
            WorldTileCoords {
                x: 2,
                y: 1,
                z: ZoomLevel::new(2),
            },
            buffer,
            vec![3],
            vec![[0.2, 0.4, 0.6, 1.0]],
            vec![],
            layer,
            "countries-boundary".into(),
            vec![entry],
        );
        let paint = transferable
            .take_feature_paint()
            .expect("feature paint survives FlatBuffer roundtrip");
        assert_eq!(paint.len(), 1);
        assert_eq!(paint[0].raw_id, Some(7));
        assert_eq!(*paint[0].properties, props);

        let bucket = transferable.to_bucket();
        let output = bucket.buffer.buffer.vertices[0];
        assert_eq!(output.position, input.position);
        assert_eq!(output.normal, input.normal);
        assert_eq!(output.distance, input.distance);
        assert_eq!(output.elevation.to_bits(), input.elevation.to_bits());
        assert_eq!(
            output.edge_distance.to_bits(),
            input.edge_distance.to_bits()
        );
        assert_eq!(output.corner_code, input.corner_code);
    }
}

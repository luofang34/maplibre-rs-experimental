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

use super::FlatBufferTransferable;
use crate::platform::singlethreaded::apc::WebMessageTag;

pub(crate) mod js_wire;

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
    let data = js_wire::encode_layer_tessellated(
        coords,
        buffer,
        feature_indices,
        feature_colors,
        feature_sort_keys,
        layer_name_str,
        style_layer_id,
        paint_bytes,
    );
    FlatBufferTransferable::from_vec(WebMessageTag::LayerTessellated, data, 0)
}

impl LayerTessellated for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerTessellated
    }

    fn style_layer_id(&self) -> &str {
        if let Some((bucket, _)) = &self.unpacked_vector {
            return &bucket.style_layer_id;
        }
        js_wire::peek_style_layer_id(self.data()).unwrap_or("")
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
        let encoded = (!entries.is_empty()).then(|| encode_feature_paint(&entries));
        pack_flat_layer_tessellated(
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            &layer_data.name,
            &style_layer_id,
            encoded.as_deref(),
        )
    }

    fn with_feature_paint(self, entries: Vec<BucketFeatureEntry>) -> Self {
        let encoded = (!entries.is_empty()).then(|| encode_feature_paint(&entries));
        let bucket = self.to_bucket();
        pack_flat_layer_tessellated(
            bucket.coords,
            bucket.buffer,
            bucket.feature_indices,
            bucket.feature_colors,
            bucket.feature_sort_keys,
            &bucket.source_layer,
            &bucket.style_layer_id,
            encoded.as_deref(),
        )
    }

    fn take_feature_paint(&mut self) -> Option<Vec<BucketFeatureEntry>> {
        if let Some((_, paint)) = &mut self.unpacked_vector {
            return paint.take();
        }
        let bytes = js_wire::peek_paint_bytes(self.data())?;
        decode_feature_paint(bytes)
    }

    fn coords(&self) -> WorldTileCoords {
        if let Some((bucket, _)) = &self.unpacked_vector {
            return bucket.coords;
        }
        js_wire::peek_coords(self.data()).unwrap_or(WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::new(0),
        })
    }

    fn is_empty(&self) -> bool {
        if let Some((bucket, _)) = &self.unpacked_vector {
            return bucket.buffer.usable_indices == 0;
        }
        js_wire::peek_usable_indices(self.data()).is_none_or(|usable| usable == 0)
    }

    fn to_bucket(mut self) -> AvailableVectorLayerBucket {
        if let Some((bucket, _)) = self.unpacked_vector.take() {
            return *bucket;
        }
        let coords = LayerTessellated::coords(&self);
        js_wire::decode_bucket_from_bytes(self.data()).unwrap_or_else(|| {
            AvailableVectorLayerBucket {
                coords,
                source_layer: String::new(),
                style_layer_id: String::new(),
                buffer: OverAlignedVertexBuffer::empty(),
                feature_indices: Vec::new(),
                feature_colors: Vec::new(),
                feature_sort_keys: Vec::new(),
            }
        })
    }
}

#[cfg(test)]
mod tests;

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
        AvailableVectorLayerBucket, LayerTessellated, TessellatedLayerPayload,
    },
};

use super::FlatBufferTransferable;
use crate::platform::singlethreaded::tag::WebMessageTag;

pub(crate) mod js_wire;
mod prepacked_wire;

impl FlatBufferTransferable {
    fn pack_flat_layer_tessellated(input: js_wire::TessellatedEncodeInput<'_>) -> Self {
        let data = js_wire::encode_layer_tessellated(input);
        Self::from_vec(WebMessageTag::LayerTessellated, data, 0)
    }
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
        Self::pack_flat_layer_tessellated(js_wire::TessellatedEncodeInput {
            coords,
            buffer,
            feature_indices,
            feature_colors,
            feature_sort_keys,
            source_layer: &layer_data.name,
            style_layer_id: &style_layer_id,
            paint_bytes: None,
            style_type: "",
            allow_prepack: false,
        })
    }

    fn build_from_with_paint(payload: TessellatedLayerPayload) -> Self {
        Self::build_from_with_paint_typed(payload, "", false)
    }

    fn build_from_with_paint_typed(
        payload: TessellatedLayerPayload,
        style_type: &str,
        allow_prepack: bool,
    ) -> Self {
        let encoded = (!payload.entries.is_empty()).then(|| encode_feature_paint(&payload.entries));
        Self::pack_flat_layer_tessellated(js_wire::TessellatedEncodeInput {
            coords: payload.coords,
            buffer: payload.buffer,
            feature_indices: payload.feature_indices,
            feature_colors: payload.feature_colors,
            feature_sort_keys: payload.feature_sort_keys,
            source_layer: &payload.layer_data.name,
            style_layer_id: &payload.style_layer_id,
            paint_bytes: encoded.as_deref(),
            style_type,
            allow_prepack,
        })
    }

    fn with_feature_paint(self, entries: Vec<BucketFeatureEntry>) -> Self {
        let encoded = (!entries.is_empty()).then(|| encode_feature_paint(&entries));
        let bucket = self.to_bucket();
        Self::pack_flat_layer_tessellated(js_wire::TessellatedEncodeInput {
            coords: bucket.coords,
            buffer: bucket.buffer,
            feature_indices: bucket.feature_indices,
            feature_colors: bucket.feature_colors,
            feature_sort_keys: bucket.feature_sort_keys,
            source_layer: &bucket.source_layer,
            style_layer_id: &bucket.style_layer_id,
            paint_bytes: encoded.as_deref(),
            style_type: "",
            allow_prepack: false,
        })
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

use std::sync::Arc;

use super::super::stream::{stream_layer_slice, ColumnarLayer};
use crate::{
    io::apc::Context,
    sdf::{assets::SymbolAtlas, tessellation::TextTessellator},
    style::{
        expression::FeatureProperties,
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        process_vector::{ProcessVectorContext, ProcessVectorError, VectorTileRequest},
        tessellation::feature_paint::FeatureStateSyncContext,
        transferables::{DefaultSymbolLayerTessellated, VectorTransferables},
    },
};

pub(super) struct ColumnarLayerContext<'a, 'b, 'ctx> {
    pub col: &'a ColumnarLayer<'b>,
    pub layer_name: &'ctx str,
    pub style: &'ctx StyleLayer,
    pub request: &'ctx VectorTileRequest,
    pub sel_slice: Option<&'ctx [usize]>,
    pub sliced_ids: Vec<Option<u64>>,
    pub sliced_props: Vec<Arc<FeatureProperties>>,
    pub state_ctx: Option<FeatureStateSyncContext>,
}

pub(super) fn tessellate_columnar_vector<T: VectorTransferables, C: Context>(
    ctx: ColumnarLayerContext<'_, '_, '_>,
    paint: &LayerPaint,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let ColumnarLayerContext {
        col,
        layer_name,
        style,
        request,
        sel_slice,
        sliced_ids,
        sliced_props,
        state_ctx,
    } = ctx;
    #[cfg(any(target_arch = "wasm32", not(test)))]
    let uniform_paint =
        crate::vector::upload_system::paint::uniform_color(style, request.style_zoom()).is_some()
            && style.type_ != "circle"
            && !crate::style::feature_state::layer_paint_reads_feature_state(style);
    let mut tessellator = crate::vector::process_vector::build_zero_tessellator(paint, request);
    tessellator.set_layer_strokes(style);
    tessellator.coordinate_scale = col.extent_scale;
    tessellator.source_ids = sliced_ids;
    tessellator.shared_properties = sliced_props;
    tessellator.feature_state_ctx = state_ctx;
    tessellator.feature_paint_retention =
        crate::vector::feature_paint::feature_paint_retention(style);
    tessellator.sort_key =
        crate::vector::tessellation::SortKeys::by(crate::vector::tessellation::sort_key_of(style));
    #[cfg(any(target_arch = "wasm32", not(test)))]
    if uniform_paint
        && tessellator.pattern_property.is_none()
        && tessellator.outline_property.is_none()
        && !tessellator.line_gradient
    {
        tessellator.omit_feature_colors = true;
    }
    if context.allow_prepack_for_style(
        request.coords,
        style,
        tessellator.sort_key.is_keyed(),
        false,
    ) {
        tessellator.maybe_enable_streaming_prepack(style.type_.as_str());
    }
    if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
        tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT vector tessellation failed");
        return context.layer_missing(&request.coords, layer_name);
    }
    tessellator.apply_sort_keys();
    let mut sort_keys = tessellator.sort_key_values();
    #[cfg(any(target_arch = "wasm32", not(test)))]
    if uniform_paint && tessellator.pattern_property.is_none() && !tessellator.line_gradient {
        tessellator.feature_colors = Vec::new();
    }
    let mut feature_indices = std::mem::take(&mut tessellator.feature_indices);
    let mut feature_colors = std::mem::take(&mut tessellator.feature_colors);
    let mut feature_paint = std::mem::take(&mut tessellator.feature_paint);
    feature_indices.shrink_to_fit();
    feature_colors.shrink_to_fit();
    sort_keys.shrink_to_fit();
    feature_paint.shrink_to_fit();
    let buffer = tessellator.into_over_aligned_buffer();
    context.layer_tessellation_finished(
        &request.coords,
        buffer,
        feature_indices,
        (feature_colors, sort_keys, feature_paint),
        col.header.clone(),
        style,
    )
}

pub(super) fn tessellate_columnar_symbol<T: VectorTransferables, C: Context>(
    ctx: ColumnarLayerContext<'_, '_, '_>,
    paint: &crate::style::layer::SymbolPaint,
    prop_count: usize,
    (layout_atlas, gpu_atlas): (&Arc<SymbolAtlas>, &Arc<SymbolAtlas>),
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let ColumnarLayerContext {
        col,
        layer_name,
        style,
        request,
        sel_slice,
        sliced_ids,
        sliced_props,
        state_ctx,
    } = ctx;
    let mut tessellator =
        TextTessellator::with_assets(paint.clone(), request.style_zoom(), layout_atlas.clone());
    tessellator.coordinate_scale = col.extent_scale;
    tessellator.overscaling = request.overscaling();
    tessellator.source_ids = sliced_ids;
    tessellator.source_indices =
        crate::sdf::tessellation::source_feature_indices(prop_count, sel_slice);
    tessellator.shared_properties = sliced_props;
    tessellator.feature_state_ctx = state_ctx.clone();
    if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
        tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT symbol tessellation failed");
        return context.layer_missing(&request.coords, layer_name);
    }
    tessellator.finish();
    if !context.index_geometry() {
        crate::sdf::tessellation::prune_symbol_features(
            &mut tessellator.features,
            style,
            state_ctx
                .as_ref()
                .and_then(|ctx| ctx.promote_property.as_deref()),
        );
    }
    tessellator.quad_buffer.vertices.shrink_to_fit();
    tessellator.quad_buffer.indices.shrink_to_fit();
    tessellator.features.shrink_to_fit();
    let has_geometry = !tessellator.quad_buffer.indices.is_empty();
    context.symbol_layer_tessellation_finished(DefaultSymbolLayerTessellated {
        coords: request.coords,
        buffer: tessellator.quad_buffer.into(),
        features: tessellator.features,
        atlas: has_geometry.then(|| gpu_atlas.clone()),
        layer_data: col.header.clone(),
        style_layer_id: style.id.clone(),
    })
}

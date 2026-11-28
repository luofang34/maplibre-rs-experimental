use std::sync::Arc;

use crate::{
    io::apc::Context,
    sdf::{
        assets::SymbolAtlas,
        tessellation::{TextTessellationInput, TextTessellator},
    },
    style::{
        expression::FeatureProperties,
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        mlt::stream::{stream_layer_slice, ColumnarLayer},
        process_vector::{ProcessVectorContext, ProcessVectorError, VectorTileRequest},
        tessellation::{
            feature_paint::FeatureStateSyncContext, ZeroTessellatorFinishOptions,
            ZeroTessellatorTileInput,
        },
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
    let uniform_paint = crate::vector::uniform_paint::uniform_color(style, request.style_zoom())
        .is_some()
        && style.type_ != "circle"
        && !crate::style::feature_state::layer_paint_reads_feature_state(style);
    #[cfg(any(target_arch = "wasm32", not(test)))]
    let omit_colors_if_uniform = Some((uniform_paint, true));
    #[cfg(not(any(target_arch = "wasm32", not(test))))]
    let omit_colors_if_uniform = None;
    #[cfg(any(target_arch = "wasm32", not(test)))]
    let clear_uniform_feature_colors = uniform_paint;
    #[cfg(not(any(target_arch = "wasm32", not(test))))]
    let clear_uniform_feature_colors = false;

    let mut tessellator = crate::vector::process_vector::build_zero_tessellator(paint, request);
    tessellator.set_layer_strokes(style);
    tessellator.configure_tile_input(ZeroTessellatorTileInput {
        coordinate_scale: col.extent_scale,
        source_ids: sliced_ids,
        shared_properties: sliced_props,
        feature_state_ctx: state_ctx,
        feature_paint_retention: crate::vector::feature_paint::feature_paint_retention(style),
        sort_key: crate::vector::tessellation::SortKeys::by(
            crate::vector::tessellation::sort_key_of(style),
        ),
        omit_colors_if_uniform,
        ..Default::default()
    });
    if context.allow_prepack_for_style(request.coords, style, tessellator.has_sort_keys(), false) {
        tessellator.maybe_enable_streaming_prepack(style.type_.as_str());
    }
    if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
        tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT vector tessellation failed");
        return context.layer_missing(&request.coords, layer_name);
    }
    let output = tessellator.into_output(ZeroTessellatorFinishOptions {
        commit_bare_geometry: false,
        apply_sort_keys: true,
        clear_uniform_feature_colors,
        shrink_to_fit: true,
    });
    context.layer_tessellation_finished(
        &request.coords,
        output.buffer,
        output.feature_indices,
        (
            output.feature_colors,
            output.sort_keys,
            output.feature_paint,
        ),
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
    let promote_prop = state_ctx
        .as_ref()
        .and_then(|ctx| ctx.promote_property.as_deref());
    let mut tessellator =
        TextTessellator::with_assets(paint.clone(), request.style_zoom(), layout_atlas.clone());
    tessellator.configure_input(TextTessellationInput {
        coordinate_scale: col.extent_scale,
        overscaling: request.overscaling(),
        source_ids: sliced_ids,
        source_indices: crate::sdf::tessellation::source_feature_indices(prop_count, sel_slice),
        shared_properties: sliced_props,
        feature_state_ctx: state_ctx.clone(),
        stream_point_runtime_keys: None,
    });
    if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
        tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT symbol tessellation failed");
        return context.layer_missing(&request.coords, layer_name);
    }
    let prune = (!context.index_geometry()).then_some((style, promote_prop));
    let output = tessellator.finish_output(false, prune);
    context.symbol_layer_tessellation_finished(DefaultSymbolLayerTessellated {
        coords: request.coords,
        buffer: output.buffer,
        features: output.features,
        atlas: output.has_geometry.then(|| gpu_atlas.clone()),
        layer_data: col.header.clone(),
        style_layer_id: style.id.clone(),
    })
}

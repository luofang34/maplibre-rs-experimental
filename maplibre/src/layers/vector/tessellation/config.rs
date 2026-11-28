//! Structured input configuration and output extraction for [`ZeroTessellator`].

use std::sync::Arc;

use geozero::FeatureProcessor as _;
use lyon::tessellation::geometry_builder::MaxIndex;

use super::{
    feature_paint::FeatureStateSyncContext,
    line::{line_elevation::LineElevationOptions, line_style::LineFeatureStyle},
    IndexDataType, ShaderVertex, SortKeys, ZeroTessellator,
};
use crate::{
    render::resource::vertex_buffer::OverAlignedVertexBuffer,
    style::{
        expression::FeatureProperties,
        layer::{PatternName, StyleProperty},
    },
    vector::feature_paint::{BucketFeatureEntry, FeaturePaintRetention},
};

/// Paint-derived configuration applied to [`ZeroTessellator`] before feeding tile geometry.
pub(crate) struct ZeroTessellatorPaintConfig {
    pub fallback_color: [f32; 4],
    pub pattern_property: Option<StyleProperty<PatternName>>,
    pub style_property: Option<StyleProperty<csscolorparser::Color>>,
    pub outline_property: Option<StyleProperty<csscolorparser::Color>>,
    pub is_line_layer: bool,
    pub line_gradient: bool,
    pub sharp_corner_offset: f32,
    pub split_corners: bool,
    pub line_feature_style: Option<LineFeatureStyle>,
    pub line_elevation: Option<LineElevationOptions>,
}

impl Default for ZeroTessellatorPaintConfig {
    fn default() -> Self {
        Self {
            fallback_color: [0.0, 0.0, 0.0, 1.0],
            pattern_property: None,
            style_property: None,
            outline_property: None,
            is_line_layer: false,
            line_gradient: false,
            sharp_corner_offset: super::line::sharp_corners::UNMAGNIFIED_OFFSET,
            split_corners: false,
            line_feature_style: None,
            line_elevation: None,
        }
    }
}

/// Per-tile source stream and feature-paint metadata applied to [`ZeroTessellator`].
pub(crate) struct ZeroTessellatorTileInput {
    pub coordinate_scale: f64,
    pub source_ids: Vec<Option<u64>>,
    pub shared_properties: Vec<Arc<FeatureProperties>>,
    pub feature_state_ctx: Option<FeatureStateSyncContext>,
    pub feature_paint_retention: FeaturePaintRetention,
    pub sort_key: SortKeys,
    pub omit_feature_colors: bool,
    /// When `Some((uniform_paint, check_outline))`, sets `omit_feature_colors` if `uniform_paint`
    /// is true and paint has no pattern, gradient, or (when `check_outline`) outline property.
    pub omit_colors_if_uniform: Option<(bool, bool)>,
    pub record_outline_entries: bool,
}

impl Default for ZeroTessellatorTileInput {
    fn default() -> Self {
        Self {
            coordinate_scale: 1.0,
            source_ids: Vec::new(),
            shared_properties: Vec::new(),
            feature_state_ctx: None,
            feature_paint_retention: FeaturePaintRetention::None,
            sort_key: SortKeys::default(),
            omit_feature_colors: false,
            omit_colors_if_uniform: None,
            record_outline_entries: false,
        }
    }
}

/// Options controlling how [`ZeroTessellator::into_output`] finalizes and trims buffers.
#[derive(Clone, Copy, Default)]
pub(crate) struct ZeroTessellatorFinishOptions {
    pub commit_bare_geometry: bool,
    pub apply_sort_keys: bool,
    pub clear_uniform_feature_colors: bool,
    pub shrink_to_fit: bool,
}

/// Finalized geometry and per-feature metadata extracted from [`ZeroTessellator`].
pub(crate) struct ZeroTessellatorOutput<I> {
    pub buffer: OverAlignedVertexBuffer<ShaderVertex, I>,
    pub feature_indices: Vec<u32>,
    pub feature_colors: Vec<[f32; 4]>,
    pub sort_keys: Vec<f32>,
    pub feature_paint: Vec<BucketFeatureEntry>,
    pub outline_entries: Vec<bool>,
}

impl<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> ZeroTessellator<I> {
    /// Applies style paint properties to this tessellator prior to tile geometry streaming.
    pub(crate) fn apply_paint_config(&mut self, cfg: ZeroTessellatorPaintConfig) {
        self.fallback_color = cfg.fallback_color;
        self.pattern_property = cfg.pattern_property;
        self.style_property = cfg.style_property;
        self.outline_property = cfg.outline_property;
        self.is_line_layer = cfg.is_line_layer;
        self.line_gradient = cfg.line_gradient;
        self.sharp_corner_offset = cfg.sharp_corner_offset;
        self.split_corners = cfg.split_corners;
        self.line_feature_style = cfg.line_feature_style;
        self.line_elevation = cfg.line_elevation;
    }

    /// Configures per-tile feature metadata, coordinate scaling, sorting, and paint retention.
    pub(crate) fn configure_tile_input(&mut self, input: ZeroTessellatorTileInput) {
        if input.coordinate_scale > 0.0 {
            self.coordinate_scale = input.coordinate_scale;
        }
        self.source_ids = input.source_ids;
        self.shared_properties = input.shared_properties;
        self.feature_state_ctx = input.feature_state_ctx;
        self.feature_paint_retention = input.feature_paint_retention;
        self.sort_key = input.sort_key;
        self.record_outline_entries = input.record_outline_entries;
        if input.omit_feature_colors {
            self.omit_feature_colors = true;
        } else if let Some((uniform_paint, check_outline)) = input.omit_colors_if_uniform
            && uniform_paint
            && self.pattern_property.is_none()
            && !self.line_gradient
            && (!check_outline || self.outline_property.is_none())
        {
            self.omit_feature_colors = true;
        }
    }

    /// Reports whether this tessellator ranks output features by a `*-sort-key` property.
    #[inline]
    pub(crate) fn has_sort_keys(&self) -> bool {
        self.sort_key.is_keyed()
    }

    /// Number of buffered tile-space vertices accumulated so far.
    #[inline]
    pub(crate) fn vertex_count(&self) -> usize {
        self.buffer.vertices.len()
    }

    /// Number of buffered tile-space triangle indices accumulated so far.
    #[inline]
    pub(crate) fn index_count(&self) -> usize {
        self.buffer.indices.len()
    }

    /// Unpacks the accumulated raw vertex and index vectors directly.
    pub(crate) fn into_raw_buffers(self) -> (Vec<ShaderVertex>, Vec<I>) {
        (self.buffer.vertices, self.buffer.indices)
    }
}

impl ZeroTessellator<IndexDataType> {
    /// Finalizes sorting, uniform paint trimming, and buffer extraction for layer completion.
    pub(crate) fn into_output(
        mut self,
        options: ZeroTessellatorFinishOptions,
    ) -> ZeroTessellatorOutput<IndexDataType> {
        if options.commit_bare_geometry
            && self.feature_indices.is_empty()
            && !self.buffer.indices.is_empty()
        {
            drop(self.feature_end(0));
        }
        let mut sort_keys = if options.apply_sort_keys {
            self.apply_sort_keys();
            self.sort_key_values()
        } else {
            Vec::new()
        };
        if options.clear_uniform_feature_colors
            && self.pattern_property.is_none()
            && !self.line_gradient
        {
            self.feature_colors = Vec::new();
        }
        let mut feature_indices = std::mem::take(&mut self.feature_indices);
        let mut feature_colors = std::mem::take(&mut self.feature_colors);
        let mut feature_paint = std::mem::take(&mut self.feature_paint);
        let outline_entries = std::mem::take(&mut self.outline_entries);
        if options.shrink_to_fit {
            feature_indices.shrink_to_fit();
            feature_colors.shrink_to_fit();
            sort_keys.shrink_to_fit();
            feature_paint.shrink_to_fit();
        }
        let buffer = self.into_over_aligned_buffer();
        ZeroTessellatorOutput {
            buffer,
            feature_indices,
            feature_colors,
            sort_keys,
            feature_paint,
            outline_entries,
        }
    }
}

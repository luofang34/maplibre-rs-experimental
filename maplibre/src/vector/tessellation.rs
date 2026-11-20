//! Tile-space geometry tessellation with per-feature paint and GPU upload padding.

use std::{cell::RefCell, sync::Arc};

pub use buffer::{OverAlignedVertexBuffer, PrepackedGpuGeometry};
pub use circle::{pack_circle_colors as circle_pack_colors, CircleOptions, CIRCLE_QUAD_INDICES};
use geozero::{error::GeozeroError, ColumnValue, FeatureProcessor, PropertyProcessor};
use lyon::{
    path::{path::Builder, Path},
    tessellation::{
        geometry_builder::MaxIndex, BuffersBuilder, FillOptions, FillRule, FillTessellator,
        FillVertex, FillVertexConstructor, StrokeOptions, StrokeTessellator, VertexBuffers,
    },
};

use crate::{
    projection::globe::subdivision::{subdivide_triangles, subdivide_triangles_from},
    render::ShaderVertex,
    style::expression::{FeatureProperties, Value},
};

mod buffer;
mod circle;
mod concave_join;
mod earcut_fill;
pub(crate) mod feature_paint;
mod geometry;
mod line_elevation;
mod line_origin;
mod line_style;
pub use line_elevation::LineElevationOptions;
pub use line_style::{LineFeatureStyle, PackedLine};
mod extrusion;
mod outline;
mod prepack;
mod sharp_corners;
pub use sharp_corners::offset_for as sharp_corner_offset;
mod sort_key;
pub(crate) mod stroke;
mod stroke_grid;
pub use extrusion::{pack_extrusion_code, unpack_datum_pair, ExtrusionOptions};
pub use sort_key::{sort_key_name, sort_key_of, SortKeys};

const DEFAULT_TOLERANCE: f32 = 0.02;

/// Vertex buffers index data type.
pub type IndexDataType = u32; // Must match INDEX_FORMAT

type GeoResult<T> = geozero::error::Result<T>;

/// Constructor for Fill and Stroke vertices.
pub struct VertexConstructor {}

impl FillVertexConstructor<ShaderVertex> for VertexConstructor {
    fn new_vertex(&mut self, vertex: FillVertex) -> ShaderVertex {
        ShaderVertex::new(vertex.position().to_array(), [0.0, 0.0])
    }
}

/// Build tessellations with vectors.
pub struct ZeroTessellator<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> {
    path_builder: RefCell<Builder>,
    path_open: bool,
    is_point: bool,
    in_polygon: bool,
    contour_start: Option<[f32; 2]>,
    contour_last: Option<[f32; 2]>,
    subdivision_granularity: u32,
    /// Whether strokes also split along the diagonals of the subdivision grid.
    terrain_grid: bool,
    clip_x_to_tile: bool,
    extend_to_north_pole: bool,
    extend_to_south_pole: bool,
    /// Factor from the source layer's coordinate extent to the 4096 tile grid.
    pub coordinate_scale: f64,
    /// When set, every coordinate becomes a circle quad and no path is built.
    circle: Option<CircleOptions>,
    /// When set, polygons become walls and a roof at the feature's height.
    extrusion: Option<ExtrusionOptions>,
    /// The layer's opacity property and the zoom it is evaluated at, multiplied into every
    /// feature's colour alpha.
    feature_opacity: Option<(crate::style::layer::StyleProperty<f32>, f64)>,

    /// When set, a line feature's colour is its length in tile units and its opacity, which the
    /// shader turns into a position along the gradient ramp.
    pub line_gradient: bool,
    /// Cap and join of stroked lines.
    pub stroke: crate::style::line_stroke::LineStroke,
    /// Entries added for the polygons of the feature being tessellated, when each is a building.
    polygon_entries: usize,
    /// Whether a sharp corner gets a vertex either side, for lines driven by distance.
    pub split_corners: bool,
    /// Tile units from a sharp corner at which a split line gets its own vertex.
    pub sharp_corner_offset: f32,
    /// A pattern that varies by feature, whose key each feature's colour carries.
    pub pattern_property:
        Option<crate::style::layer::StyleProperty<crate::style::layer::PatternName>>,
    /// A `line-join` that varies by feature, which replaces the stroke's own.
    pub join_property: Option<crate::style::layer::StyleProperty<String>>,
    line_length: f32,

    /// Accumulated tile-space vertices and indices, without upload padding.
    pub buffer: VertexBuffers<ShaderVertex, I>,

    /// Vertex count contributed by each completed feature, in processing order.
    pub feature_indices: Vec<u32>,
    /// Attributes of the feature being processed; cleared after its color is evaluated.
    pub feature_properties: FeatureProperties,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
    /// Encoded-sRGB colors with straight alpha for completed features, including feature opacity.
    pub feature_colors: Vec<[f32; 4]>,
    /// Skips per-feature `feature_colors` evaluation/storage when layer paint is uniform on web.
    pub(crate) omit_feature_colors: bool,
    /// Records whether each entry in `feature_indices` is an outline (`true`) or main fill (`false`).
    pub(crate) record_outline_entries: bool,
    /// Parallel to `feature_indices` when `record_outline_entries` is set.
    pub(crate) outline_entries: Vec<bool>,
    /// Color used when no color property is supplied or its evaluation fails.
    pub fallback_color: [f32; 4],
    /// Color expression evaluated against each feature's attributes at `zoom`.
    pub style_property: Option<crate::style::layer::StyleProperty<csscolorparser::Color>>,
    /// When true, polygon geometry is tessellated as strokes (outlines) instead of fills.
    /// This is used when a line-type style layer references polygon source geometry.
    pub is_line_layer: bool,
    /// Set when a line's width or offset varies by feature; packed into each stroke vertex.
    pub line_feature_style: Option<LineFeatureStyle>,
    /// Set when `line-elevation` lifts stroked paths to 3D altitudes in metres.
    pub line_elevation: Option<LineElevationOptions>,
    /// Colour of a one-pixel outline drawn along the edges of each filled polygon.
    pub outline_property: Option<crate::style::layer::StyleProperty<csscolorparser::Color>>,
    /// The outline of the feature being processed, drawn after its fill.
    outline: VertexBuffers<ShaderVertex, I>,
    /// Pre-encoded outline vertices accumulated per-polygon when `streaming_prepack` is active.
    pub(super) outline_prepack: Vec<crate::render::shaders::GpuShaderVertex>,
    /// Orders features by `*-sort-key`, so the greater keys draw on top.
    pub sort_key: SortKeys,
    /// The sort key of each entry of `feature_indices`, set by `apply_sort_keys`.
    pub entry_sort_keys: Vec<f32>,
    /// Source feature IDs indexed by position in the layer's feature stream.
    pub(crate) source_ids: Vec<Option<u64>>,
    /// Pre-decoded shared feature property maps indexed by position in the feature stream.
    pub(crate) shared_properties: Vec<Arc<FeatureProperties>>,
    current_shared_props: Option<Arc<FeatureProperties>>,
    /// Source `promoteId` and active runtime `feature-state` snapshot for this layer.
    pub(crate) feature_state_ctx: Option<feature_paint::FeatureStateSyncContext>,
    current_raw_id: Option<u64>,
    state_synced: bool,
    /// Retained per-entry feature identity and attributes for in-place GPU paint refresh.
    pub(crate) feature_paint: Vec<crate::vector::feature_paint::BucketFeatureEntry>,
    /// The one empty property map every slot-kind entry of this bucket shares.
    slot_kind_properties: Arc<FeatureProperties>,
    /// What each retained entry keeps for this layer's paint.
    pub(crate) feature_paint_retention: crate::vector::feature_paint::FeaturePaintRetention,
    current_index: usize,
    streaming_prepack: Option<prepack::StreamingPrepack>,
    current_extrusion_heights: Option<extrusion::ExtrusionHeights>,
    earcut: earcut_fill::PolygonEarcut,
    fill_tessellator: FillTessellator,
    stroke_tessellator: StrokeTessellator,
}

impl<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> Default
    for ZeroTessellator<I>
{
    fn default() -> Self {
        Self {
            path_builder: RefCell::new(Path::builder()),
            buffer: VertexBuffers::new(),
            feature_indices: Vec::new(),
            feature_properties: FeatureProperties::default(),
            zoom: 0.0,
            feature_colors: Vec::new(),
            omit_feature_colors: false,
            record_outline_entries: false,
            outline_entries: Vec::new(),
            fallback_color: [0.0, 0.0, 0.0, 1.0],
            style_property: None,
            is_line_layer: false,
            line_feature_style: None,
            line_elevation: None,
            line_gradient: false,
            stroke: Default::default(),
            join_property: None,
            split_corners: false,
            sharp_corner_offset: sharp_corners::UNMAGNIFIED_OFFSET,
            polygon_entries: 0,
            pattern_property: None,
            line_length: 0.0,
            current_index: 0,
            path_open: false,
            is_point: false,
            in_polygon: false,
            contour_start: None,
            contour_last: None,
            subdivision_granularity: 1,
            terrain_grid: false,
            clip_x_to_tile: false,
            extend_to_north_pole: false,
            extend_to_south_pole: false,
            coordinate_scale: 1.0,
            circle: None,
            extrusion: None,
            feature_opacity: None,
            outline_property: None,
            outline: VertexBuffers::new(),
            outline_prepack: Vec::new(),
            sort_key: Default::default(),
            entry_sort_keys: Vec::new(),
            source_ids: Vec::new(),
            shared_properties: Vec::new(),
            current_shared_props: None,
            feature_state_ctx: None,
            current_raw_id: None,
            state_synced: false,
            feature_paint: Vec::new(),
            slot_kind_properties: Arc::default(),
            feature_paint_retention: crate::vector::feature_paint::FeaturePaintRetention::None,
            streaming_prepack: None,
            current_extrusion_heights: None,
            earcut: earcut_fill::PolygonEarcut::default(),
            fill_tessellator: FillTessellator::new(),
            stroke_tessellator: StrokeTessellator::new(),
        }
    }
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    /// Multiplies the layer's opacity property, evaluated per feature at `zoom`, into every
    /// feature colour, as GL JS folds `fill-opacity`, `line-opacity` and `circle-opacity`.
    pub fn with_feature_opacity(
        mut self,
        opacity: Option<crate::style::layer::StyleProperty<f32>>,
        zoom: f64,
    ) -> Self {
        self.feature_opacity = opacity.map(|opacity| (opacity, zoom));
        self.zoom = zoom;
        self
    }

    /// Raises polygons to the feature's height instead of filling them flat.
    pub fn with_extrusion(mut self, options: ExtrusionOptions) -> Self {
        self.extrusion = Some(options);
        self
    }

    /// Configures geometry subdivision for a globe tile.
    pub fn with_globe_subdivision(
        mut self,
        granularity: u32,
        clip_x_to_tile: bool,
        extend_to_north_pole: bool,
        extend_to_south_pole: bool,
    ) -> Self {
        self.subdivision_granularity = granularity.max(1);
        self.clip_x_to_tile = clip_x_to_tile;
        self.extend_to_north_pole = extend_to_north_pole;
        self.extend_to_south_pole = extend_to_south_pole;
        self
    }

    /// Streams geometry directly into compact/encoded GPU vertex storage after each polygon or stroke
    /// when `style_type` and layer paint flags support prepacking.
    pub(crate) fn maybe_enable_streaming_prepack(&mut self, style_type: &str) {
        if self.pattern_property.is_none() && !self.sort_key.is_keyed() {
            let allow_compact = self.omit_feature_colors
                && self.feature_paint_retention
                    == crate::vector::feature_paint::FeaturePaintRetention::None
                && style_type == "fill-extrusion"
                && self.extrusion.as_ref().is_some_and(|e| !e.has_pattern);
            self.enable_streaming_prepack(style_type, allow_compact);
        }
    }

    /// Streams geometry directly into compact/encoded GPU vertex storage after each polygon or stroke.
    pub(crate) fn enable_streaming_prepack(
        &mut self,
        style_type: &str,
        allow_compact_extrusion: bool,
    ) {
        self.streaming_prepack =
            prepack::StreamingPrepack::new(style_type, allow_compact_extrusion);
    }

    #[inline]
    fn total_vertex_count(&self) -> usize {
        self.streaming_prepack
            .as_ref()
            .map_or(0, prepack::StreamingPrepack::logical_len)
            + self.buffer.vertices.len()
    }

    fn flush_streaming_prepack(&mut self, index_start: usize) {
        let Some(prepack) = &mut self.streaming_prepack else {
            return;
        };
        if self.buffer.vertices.is_empty() {
            return;
        }
        let base = prepack.logical_len();
        if base > 0 {
            for idx in &mut self.buffer.indices[index_start..] {
                *idx = I::from(lyon::tessellation::VertexId::from_usize(
                    base + (*idx).into() as usize,
                ));
            }
        }
        prepack.push_vertices(&self.buffer.vertices);
        self.buffer.vertices.clear();
    }

    fn update_feature_indices(&mut self) {
        let next_index = self.total_vertex_count();
        let indices = (next_index - self.current_index) as u32;
        self.feature_indices.push(indices);
        self.current_index = next_index;
    }

    fn end(&mut self, close: bool) -> GeoResult<()> {
        if self.path_open {
            if close {
                self.subdivide_closing_segment()?;
            }
            self.path_builder.borrow_mut().end(close);
            self.path_open = false;
        }
        self.contour_start = None;
        self.contour_last = None;
        Ok(())
    }

    fn tessellate_extrusion(&mut self, options: &ExtrusionOptions) -> GeoResult<()> {
        let index_start = self.buffer.indices.len();
        let mut path = self.path_builder.replace(Path::builder()).build();
        if let Some(buffer) = options.clip_buffer {
            path = extrusion::clipped(&path, buffer);
        }
        let heights = if let Some(cached) = self.current_extrusion_heights {
            cached
        } else {
            let evaluated =
                extrusion::ExtrusionHeights::for_feature(options, self.props(), self.zoom);
            self.current_extrusion_heights = Some(evaluated);
            evaluated
        };
        let subdivision = crate::projection::globe::subdivision::FillSubdivisionOptions {
            granularity: self.subdivision_granularity,
            clip_x_to_tile: self.clip_x_to_tile,
            extend_to_north_pole: self.extend_to_north_pole,
            extend_to_south_pole: self.extend_to_south_pole,
        };
        extrusion::extrude_with_tessellator(
            (&mut self.fill_tessellator, &mut self.earcut),
            &path,
            heights,
            &mut self.buffer,
            (
                DEFAULT_TOLERANCE,
                |buffer: &mut VertexBuffers<ShaderVertex, I>, start| {
                    subdivide_triangles(buffer, start, subdivision)
                        .map_err(|error| error.to_string())
                },
            ),
        )
        .map_err(GeozeroError::Geometry)?;
        self.flush_streaming_prepack(index_start);
        // Each polygon of a multipolygon is a building of its own, standing on its own ground.
        self.update_feature_indices();
        self.polygon_entries += 1;
        Ok(())
    }

    fn tessellate_fill(&mut self) -> GeoResult<()> {
        if let Some(options) = self.extrusion.clone() {
            return self.tessellate_extrusion(&options);
        }
        let path_builder = self.path_builder.replace(Path::builder());
        let vertex_start = self.buffer.vertices.len();
        let index_start = self.buffer.indices.len();
        let path = path_builder.build();
        if self.outline_property.is_some() {
            let outline_vertex_start = self.outline.vertices.len();
            let outline_index_start = self.outline.indices.len();
            self.stroke_tessellator
                .tessellate_path(
                    &path,
                    &StrokeOptions::tolerance(DEFAULT_TOLERANCE)
                        .with_line_width(outline::OUTLINE_WIDTH)
                        .with_miter_limit(outline::OUTLINE_MITER_LIMIT),
                    &mut BuffersBuilder::new(&mut self.outline, VertexConstructor {}),
                )
                .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
            concave_join::split_unclipped_concave_joins(
                &mut self.outline,
                outline_vertex_start,
                outline_index_start,
                outline::MAX_OUTLINE_NORMAL,
            );
            self.flush_polygon_outline_prepack(outline_index_start);
        }

        if !self
            .earcut
            .try_triangulate(&path, false, &mut self.buffer, |pos| {
                ShaderVertex::new(pos, [0.0, 0.0])
            })
        {
            self.fill_tessellator
                .tessellate_path(
                    &path,
                    &FillOptions::tolerance(DEFAULT_TOLERANCE).with_fill_rule(FillRule::NonZero),
                    &mut BuffersBuilder::new(&mut self.buffer, VertexConstructor {}),
                )
                .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        }
        subdivide_triangles_from(
            &mut self.buffer,
            vertex_start,
            index_start,
            crate::projection::globe::subdivision::FillSubdivisionOptions {
                granularity: self.subdivision_granularity,
                clip_x_to_tile: self.clip_x_to_tile,
                extend_to_north_pole: self.extend_to_north_pole,
                extend_to_south_pole: self.extend_to_south_pole,
            },
        )
        .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        self.flush_streaming_prepack(index_start);
        Ok(())
    }
}

impl ZeroTessellator<IndexDataType> {
    /// Finalizes tessellated geometry into an upload-ready [`OverAlignedVertexBuffer`].
    pub(crate) fn into_over_aligned_buffer(
        mut self,
    ) -> OverAlignedVertexBuffer<ShaderVertex, IndexDataType> {
        if let Some(mut prepack) = self.streaming_prepack.take() {
            if !self.buffer.vertices.is_empty() {
                prepack.push_vertices(&self.buffer.vertices);
                self.buffer.vertices = Vec::new();
            }
            return prepack.finish(self.buffer.indices);
        }
        self.buffer.vertices.shrink_to_fit();
        self.buffer.indices.shrink_to_fit();
        self.buffer.into()
    }
}

impl<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> PropertyProcessor
    for ZeroTessellator<I>
{
    fn property(
        &mut self,
        _idx: usize,
        name: &str,
        value: &ColumnValue,
    ) -> geozero::error::Result<bool> {
        if self.current_shared_props.is_none() {
            if let Some(value) = property_value(value) {
                self.feature_properties.insert(name.to_string(), value);
            }
        }
        Ok(false)
    }
}

impl<I> FeatureProcessor for ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    fn feature_begin(&mut self, idx: u64) -> geozero::error::Result<()> {
        self.begin_feature(idx);
        Ok(())
    }

    fn geometry_begin(&mut self) -> geozero::error::Result<()> {
        self.ensure_feature_state_synced();
        Ok(())
    }

    fn feature_end(&mut self, _idx: u64) -> geozero::error::Result<()> {
        self.finish_feature();
        Ok(())
    }
}

#[cfg(test)]
mod tests;

mod property;
pub(crate) use property::property_value;

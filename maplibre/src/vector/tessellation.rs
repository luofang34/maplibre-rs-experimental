//! Tile-space geometry tessellation with per-feature paint and GPU upload padding.

use std::cell::RefCell;

use bytemuck::Pod;
pub use circle::{CircleOptions, CIRCLE_QUAD_INDICES};
use geozero::{error::GeozeroError, ColumnValue, FeatureProcessor, PropertyProcessor};
use lyon::{
    geom,
    path::{path::Builder, Path},
    tessellation::{
        geometry_builder::MaxIndex, BuffersBuilder, FillOptions, FillRule, FillTessellator,
        FillVertex, FillVertexConstructor, StrokeOptions, StrokeTessellator, VertexBuffers,
    },
};

use crate::{
    projection::globe::subdivision::{subdivide_line_segment, subdivide_triangles},
    render::ShaderVertex,
    style::{
        expression::{FeatureProperties, Value},
        line_stroke::{LineCap, LineJoin},
    },
};

/// The length of the path's segments in tile units.
fn path_length(path: &Path) -> f32 {
    path.iter()
        .map(|event| match event {
            lyon::path::Event::Line { from, to } => (to - from).length(),
            lyon::path::Event::End {
                last,
                first,
                close: true,
            } => (first - last).length(),
            _ => 0.0,
        })
        .sum()
}

mod circle;
mod geometry;
mod line_origin;
mod line_style;
pub use line_style::{LineFeatureStyle, PackedLine};
mod extrusion;
pub use extrusion::ExtrusionOptions;

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

impl lyon::tessellation::StrokeVertexConstructor<ShaderVertex> for VertexConstructor {
    fn new_vertex(&mut self, vertex: lyon::tessellation::StrokeVertex) -> ShaderVertex {
        ShaderVertex::new(vertex.position().to_array(), [0.0, 0.0])
    }
}

/// Outline width in tile units: about one pixel of a 512 px tile.
const OUTLINE_WIDTH: f32 = 8.0;

/// Geometry with a draw-index count separate from any GPU copy-alignment padding.
/// Conversion from `VertexBuffers` pads indices and requires copy-aligned vertex bytes.
#[derive(Clone)]
pub struct OverAlignedVertexBuffer<V, I> {
    /// Vertex and index storage, including any upload padding.
    pub buffer: VertexBuffers<V, I>,
    /// Number of leading indices to draw, excluding padding.
    pub usable_indices: u32,
}

impl<V, I> OverAlignedVertexBuffer<V, I> {
    /// Creates empty geometry with no drawable indices.
    pub fn empty() -> Self {
        Self {
            buffer: VertexBuffers::with_capacity(0, 0),
            usable_indices: 0,
        }
    }

    /// Reconstructs buffers without adding padding or validating `usable_indices`.
    /// Callers must supply copy-aligned storage and a draw count within the index buffer.
    pub fn from_iters<IV, II>(vertices: IV, indices: II, usable_indices: u32) -> Self
    where
        IV: IntoIterator<Item = V>,
        II: IntoIterator<Item = I>,
        IV::IntoIter: ExactSizeIterator,
        II::IntoIter: ExactSizeIterator,
    {
        let vertices = vertices.into_iter();
        let indices = indices.into_iter();
        let mut buffers = VertexBuffers::with_capacity(vertices.len(), indices.len());
        buffers.vertices.extend(vertices);
        buffers.indices.extend(indices);
        Self {
            buffer: buffers,
            usable_indices,
        }
    }
}

impl<V: Pod, I: Pod> From<VertexBuffers<V, I>> for OverAlignedVertexBuffer<V, I> {
    fn from(mut buffer: VertexBuffers<V, I>) -> Self {
        let usable_indices = buffer.indices.len() as u32;
        buffer.align_vertices();
        buffer.align_indices();
        Self {
            buffer,
            usable_indices,
        }
    }
}

trait Align<V: Pod, I: Pod> {
    fn align_vertices(&mut self);
    fn align_indices(&mut self);
}

impl<V: Pod, I: Pod> Align<V, I> for VertexBuffers<V, I> {
    fn align_vertices(&mut self) {
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let stride = std::mem::size_of::<V>() as wgpu::BufferAddress;
        let unpadded_bytes = self.vertices.len() as wgpu::BufferAddress * stride;
        let padding_bytes = (align - unpadded_bytes % align) % align;

        if padding_bytes != 0 {
            panic!(
                "vertices are always aligned to wgpu::COPY_BUFFER_ALIGNMENT \
                    because GpuVertexUniform is aligned"
            )
        }
    }

    fn align_indices(&mut self) {
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let stride = std::mem::size_of::<I>() as wgpu::BufferAddress;
        let unpadded_bytes = self.indices.len() as wgpu::BufferAddress * stride;
        let padding_bytes = (align - unpadded_bytes % align) % align;
        let overpad = padding_bytes.div_ceil(stride);

        for _ in 0..overpad {
            self.indices.push(I::zeroed());
        }
    }
}

/// Build tessellations with vectors.
pub struct ZeroTessellator<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> {
    path_builder: RefCell<Builder>,
    path_open: bool,
    is_point: bool,
    contour_start: Option<[f32; 2]>,
    contour_last: Option<[f32; 2]>,
    subdivision_granularity: u32,
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
    /// Color used when no color property is supplied or its evaluation fails.
    pub fallback_color: [f32; 4],
    /// Color expression evaluated against each feature's attributes at `zoom`.
    pub style_property: Option<crate::style::layer::StyleProperty<csscolorparser::Color>>,
    /// When true, polygon geometry is tessellated as strokes (outlines) instead of fills.
    /// This is used when a line-type style layer references polygon source geometry.
    pub is_line_layer: bool,
    /// Set when a line's width or offset varies by feature; packed into each stroke vertex.
    pub line_feature_style: Option<LineFeatureStyle>,
    /// Colour of a one-pixel outline drawn along the edges of each filled polygon.
    pub outline_property: Option<crate::style::layer::StyleProperty<csscolorparser::Color>>,
    /// The outline of the feature being processed, drawn after its fill.
    outline: VertexBuffers<ShaderVertex, I>,
    current_index: usize,
}

impl<I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex> Default
    for ZeroTessellator<I>
{
    fn default() -> Self {
        Self {
            path_builder: RefCell::new(Path::builder()),
            buffer: VertexBuffers::new(),
            feature_indices: Vec::new(),
            feature_properties: FeatureProperties::new(),
            zoom: 0.0,
            feature_colors: Vec::new(),
            fallback_color: [0.0, 0.0, 0.0, 1.0],
            style_property: None,
            is_line_layer: false,
            line_feature_style: None,
            line_gradient: false,
            stroke: Default::default(),
            line_length: 0.0,
            current_index: 0,
            path_open: false,
            is_point: false,
            contour_start: None,
            contour_last: None,
            subdivision_granularity: 1,
            clip_x_to_tile: false,
            extend_to_north_pole: false,
            extend_to_south_pole: false,
            coordinate_scale: 1.0,
            circle: None,
            extrusion: None,
            feature_opacity: None,
            outline_property: None,
            outline: VertexBuffers::new(),
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

    fn update_feature_indices(&mut self) {
        let next_index = self.buffer.vertices.len();
        let indices = (next_index - self.current_index) as u32;
        self.feature_indices.push(indices);
        self.current_index = next_index;
    }

    fn tessellate_strokes(&mut self) -> GeoResult<()> {
        let path = self.path_builder.replace(Path::builder()).build();
        self.line_length = self.line_length.max(path_length(&path));
        // A gradient measures its progress along the whole line, not from the tile.
        let constructor = line_origin::StrokeOrigins {
            origins: if self.line_gradient {
                Vec::new()
            } else {
                line_origin::entry_distances(&path)
            },
            packed: self
                .line_feature_style
                .as_ref()
                .map_or(PackedLine::LAYER, |style| {
                    style.pack(&self.feature_properties)
                }),
        };

        StrokeTessellator::new()
            .tessellate_path(
                &path,
                &StrokeOptions::tolerance(DEFAULT_TOLERANCE)
                    .with_line_cap(match self.stroke.cap {
                        LineCap::Butt => lyon::tessellation::LineCap::Butt,
                        LineCap::Round => lyon::tessellation::LineCap::Round,
                        LineCap::Square => lyon::tessellation::LineCap::Square,
                    })
                    .with_line_join(match self.stroke.join {
                        LineJoin::Miter => lyon::tessellation::LineJoin::Miter,
                        LineJoin::Bevel => lyon::tessellation::LineJoin::Bevel,
                        LineJoin::Round => lyon::tessellation::LineJoin::Round,
                    })
                    .with_miter_limit(self.stroke.miter_limit),
                &mut BuffersBuilder::new(&mut self.buffer, constructor),
            )
            .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        Ok(())
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
        let path = self.path_builder.replace(Path::builder()).build();
        let evaluate = |property: &Option<crate::style::layer::StyleProperty<f32>>| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(&self.feature_properties, self.zoom))
                .unwrap_or(0.0)
        };
        let height = evaluate(&options.height);
        let base = evaluate(&options.base).max(0.0);
        extrusion::extrude(&path, (base, height), &mut self.buffer, DEFAULT_TOLERANCE)
            .map_err(|error| GeozeroError::Geometry(error.to_string()))
    }

    fn tessellate_fill(&mut self) -> GeoResult<()> {
        if let Some(options) = self.extrusion.clone() {
            return self.tessellate_extrusion(&options);
        }
        let path_builder = self.path_builder.replace(Path::builder());
        let index_start = self.buffer.indices.len();
        let path = path_builder.build();
        if self.outline_property.is_some() {
            StrokeTessellator::new()
                .tessellate_path(
                    &path,
                    &StrokeOptions::tolerance(DEFAULT_TOLERANCE).with_line_width(OUTLINE_WIDTH),
                    &mut BuffersBuilder::new(&mut self.outline, VertexConstructor {}),
                )
                .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        }

        FillTessellator::new()
            .tessellate_path(
                &path,
                &FillOptions::tolerance(DEFAULT_TOLERANCE).with_fill_rule(FillRule::NonZero),
                &mut BuffersBuilder::new(&mut self.buffer, VertexConstructor {}),
            )
            .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        subdivide_triangles(
            &mut self.buffer,
            index_start,
            crate::projection::globe::subdivision::FillSubdivisionOptions {
                granularity: self.subdivision_granularity,
                clip_x_to_tile: self.clip_x_to_tile,
                extend_to_north_pole: self.extend_to_north_pole,
                extend_to_south_pole: self.extend_to_south_pole,
            },
        )
        .map_err(|error| GeozeroError::Geometry(error.to_string()))
    }

    /// Draws the pending outline as a feature of its own, after the fill it belongs to.
    fn append_outline(&mut self) {
        let outline = std::mem::replace(&mut self.outline, VertexBuffers::new());
        let Some(property) = &self.outline_property else {
            return;
        };
        let Some(colour) = property.evaluate_for(&self.feature_properties, self.zoom) else {
            return;
        };
        if outline.vertices.is_empty() {
            return;
        }
        let base = self.buffer.vertices.len();
        self.buffer.vertices.extend(outline.vertices);
        self.buffer
            .indices
            .extend(outline.indices.into_iter().map(|index| {
                I::from(lyon::tessellation::VertexId::from_usize(
                    base + index.into() as usize,
                ))
            }));
        self.update_feature_indices();
        let opacity = self
            .feature_opacity
            .as_ref()
            .map_or(1.0, |(opacity, zoom)| {
                opacity
                    .evaluate_for(&self.feature_properties, *zoom)
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            });
        self.feature_colors.push([
            colour.r as f32,
            colour.g as f32,
            colour.b as f32,
            colour.a as f32 * opacity,
        ]);
    }

    fn append_coordinate(&mut self, coordinate: [f32; 2]) -> GeoResult<()> {
        if let Some(previous) = self.contour_last {
            let points = subdivide_line_segment(previous, coordinate, self.subdivision_granularity)
                .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
            for point in points {
                self.path_builder
                    .borrow_mut()
                    .line_to(geom::point(point[0], point[1]));
            }
        } else {
            self.path_builder
                .borrow_mut()
                .begin(geom::point(coordinate[0], coordinate[1]));
            self.contour_start = Some(coordinate);
            self.path_open = true;
        }
        self.contour_last = Some(coordinate);
        Ok(())
    }

    fn subdivide_closing_segment(&mut self) -> GeoResult<()> {
        let (Some(start), Some(last)) = (self.contour_start, self.contour_last) else {
            return Ok(());
        };
        let mut points = subdivide_line_segment(last, start, self.subdivision_granularity)
            .map_err(|error| GeozeroError::Geometry(error.to_string()))?;
        points.pop();
        for point in points {
            self.path_builder
                .borrow_mut()
                .line_to(geom::point(point[0], point[1]));
        }
        Ok(())
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
        if let Some(value) = property_value(value) {
            self.feature_properties.insert(name.to_string(), value);
        }
        Ok(false)
    }
}

impl<I> FeatureProcessor for ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    fn feature_end(&mut self, _idx: u64) -> geozero::error::Result<()> {
        self.update_feature_indices();
        let mut color = if let Some(style) = &self.style_property {
            if let Some(c) = style.evaluate_for(&self.feature_properties, self.zoom) {
                [c.r as f32, c.g as f32, c.b as f32, c.a as f32]
            } else {
                tracing::debug!(
                    "Style evaluation failed for feature properties: {:?}, style: {:?}",
                    self.feature_properties,
                    style
                );
                self.fallback_color
            }
        } else {
            self.fallback_color
        };
        if self.line_gradient {
            color = [self.line_length, 0.0, 0.0, 1.0];
            self.line_length = 0.0;
        }
        if let Some((opacity, zoom)) = &self.feature_opacity {
            color[3] *= opacity
                .evaluate_for(&self.feature_properties, *zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
        }

        self.feature_colors.push(color);
        self.append_outline();
        self.feature_properties.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests;

mod property;
pub(crate) use property::property_value;

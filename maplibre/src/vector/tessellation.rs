//! Tessellation for lines and polygons is implemented here.

use std::cell::RefCell;

use bytemuck::Pod;
use geozero::{
    error::GeozeroError, ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor,
};
use lyon::{
    geom,
    path::{path::Builder, Path},
    tessellation::{
        geometry_builder::MaxIndex, BuffersBuilder, FillOptions, FillRule, FillTessellator,
        FillVertex, FillVertexConstructor, StrokeOptions, StrokeTessellator, StrokeVertex,
        StrokeVertexConstructor, VertexBuffers,
    },
};

pub use circle::{CircleOptions, CIRCLE_QUAD_INDICES};

use crate::{
    projection::globe::subdivision::{subdivide_line_segment, subdivide_triangles},
    render::ShaderVertex,
    style::expression::{FeatureProperties, Value},
};

mod circle;

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

impl StrokeVertexConstructor<ShaderVertex> for VertexConstructor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> ShaderVertex {
        ShaderVertex::new(
            vertex.position_on_path().to_array(),
            vertex.normal().to_array(),
        )
    }
}

/// Vertex buffer which includes additional padding to fulfill the `wgpu::COPY_BUFFER_ALIGNMENT`.
#[derive(Clone)]
pub struct OverAlignedVertexBuffer<V, I> {
    pub buffer: VertexBuffers<V, I>,
    pub usable_indices: u32,
}

impl<V, I> OverAlignedVertexBuffer<V, I> {
    pub fn empty() -> Self {
        Self {
            buffer: VertexBuffers::with_capacity(0, 0),
            usable_indices: 0,
        }
    }

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
        let overpad = (padding_bytes + stride - 1) / stride; // Divide by stride but round up

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
    /// The layer's opacity property and the zoom it is evaluated at, multiplied into every
    /// feature's colour alpha.
    feature_opacity: Option<(crate::style::layer::StyleProperty<f32>, f64)>,

    pub buffer: VertexBuffers<ShaderVertex, I>,

    pub feature_indices: Vec<u32>,
    pub feature_properties: FeatureProperties,
    /// Zoom of the tile, at which zoom-driven properties are evaluated.
    pub zoom: f64,
    pub feature_colors: Vec<[f32; 4]>,
    pub fallback_color: [f32; 4],
    pub style_property: Option<crate::style::layer::StyleProperty<csscolorparser::Color>>,
    /// When true, polygon geometry is tessellated as strokes (outlines) instead of fills.
    /// This is used when a line-type style layer references polygon source geometry.
    pub is_line_layer: bool,
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
            feature_opacity: None,
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

    /// Stores current indices to the output. That way we know which vertices correspond to which
    /// feature in the output.
    fn update_feature_indices(&mut self) {
        let next_index = self.buffer.vertices.len();
        let indices = (next_index - self.current_index) as u32;
        self.feature_indices.push(indices);
        self.current_index = next_index;
    }

    fn tessellate_strokes(&mut self) -> GeoResult<()> {
        let path_builder = self.path_builder.replace(Path::builder());

        StrokeTessellator::new()
            .tessellate_path(
                &path_builder.build(),
                &StrokeOptions::tolerance(DEFAULT_TOLERANCE),
                &mut BuffersBuilder::new(&mut self.buffer, VertexConstructor {}),
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

    fn tessellate_fill(&mut self) -> GeoResult<()> {
        let path_builder = self.path_builder.replace(Path::builder());
        let index_start = self.buffer.indices.len();

        FillTessellator::new()
            .tessellate_path(
                &path_builder.build(),
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

impl<I> GeomProcessor for ZeroTessellator<I>
where
    I: std::ops::Add + From<lyon::tessellation::VertexId> + MaxIndex + Copy + Into<u32>,
{
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> GeoResult<()> {
        let scale = self.coordinate_scale;
        let coordinate = [(x * scale) as f32, (y * scale) as f32];
        if self.circle.is_some() {
            self.emit_circle(coordinate[0], coordinate[1]);
        } else if !self.is_point {
            self.append_coordinate(coordinate)?;
        }
        Ok(())
    }

    fn point_begin(&mut self, _idx: usize) -> GeoResult<()> {
        // log::info!("point_begin");
        self.is_point = true;
        Ok(())
    }

    fn point_end(&mut self, _idx: usize) -> GeoResult<()> {
        // log::info!("point_end");
        self.is_point = false;
        Ok(())
    }

    fn multipoint_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        // log::info!("multipoint_begin");
        Ok(())
    }

    fn multipoint_end(&mut self, _idx: usize) -> GeoResult<()> {
        // log::info!("multipoint_end");
        Ok(())
    }

    fn linestring_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GeoResult<()> {
        // log::info!("linestring_begin");
        Ok(())
    }

    fn linestring_end(&mut self, tagged: bool, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.end(false)?;

        if tagged {
            self.tessellate_strokes()?;
        }
        Ok(())
    }

    fn multilinestring_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        // log::info!("multilinestring_begin");
        Ok(())
    }

    fn multilinestring_end(&mut self, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.tessellate_strokes()?;
        Ok(())
    }

    fn polygon_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GeoResult<()> {
        // log::info!("polygon_begin");
        Ok(())
    }

    fn polygon_end(&mut self, tagged: bool, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        self.end(true)?;
        if tagged {
            if self.is_line_layer {
                self.tessellate_strokes()?;
            } else {
                self.tessellate_fill()?;
            }
        }
        Ok(())
    }

    fn multipolygon_begin(&mut self, _size: usize, _idx: usize) -> GeoResult<()> {
        // log::info!("multipolygon_begin");
        Ok(())
    }

    fn multipolygon_end(&mut self, _idx: usize) -> GeoResult<()> {
        if self.circle.is_some() {
            return Ok(());
        }
        if self.is_line_layer {
            self.tessellate_strokes()?;
        } else {
            self.tessellate_fill()?;
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
        if let Some((opacity, zoom)) = &self.feature_opacity {
            color[3] *= opacity
                .evaluate_for(&self.feature_properties, *zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
        }

        self.feature_colors.push(color);
        self.feature_properties.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests;

/// The typed value of a feature property, as an expression sees it.
fn property_value(value: &ColumnValue) -> Option<Value> {
    Some(match value {
        ColumnValue::Bool(flag) => Value::Bool(*flag),
        ColumnValue::Byte(number) => Value::Number(f64::from(*number)),
        ColumnValue::UByte(number) => Value::Number(f64::from(*number)),
        ColumnValue::Short(number) => Value::Number(f64::from(*number)),
        ColumnValue::UShort(number) => Value::Number(f64::from(*number)),
        ColumnValue::Int(number) => Value::Number(f64::from(*number)),
        ColumnValue::UInt(number) => Value::Number(f64::from(*number)),
        ColumnValue::Long(number) => Value::Number(*number as f64),
        ColumnValue::ULong(number) => Value::Number(*number as f64),
        ColumnValue::Float(number) => Value::Number(f64::from(*number)),
        ColumnValue::Double(number) => Value::Number(*number),
        ColumnValue::String(text) | ColumnValue::DateTime(text) => {
            Value::String((*text).to_string())
        }
        ColumnValue::Json(text) => serde_json::from_str::<serde_json::Value>(text)
            .map(|json| Value::from_json(&json))
            .unwrap_or_else(|_| Value::String((*text).to_string())),
        ColumnValue::Binary(_) => return None,
    })
}

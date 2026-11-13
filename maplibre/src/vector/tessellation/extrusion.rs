//! Walls, roofs and optional floor caps of extruded polygons.
//!
//! A vertex carries its place in the fields the fill and line shaders use otherwise: the
//! wall's lighting normal in `normal`, scaled to length two on the top edge so the shader can
//! tell it from the bottom edge, the base in `distance`, and the roof height in `elevation`.
//! A roof vertex has the zero normal; a floating floor cap vertex carries [`FLOOR_NORMAL`].
//! `corner_code` packs, as [`next_zoom_code`] and [`pack_extrusion_code`] do, how much the
//! vertex rises one zoom above the tile's together with its `(base_datum, height_datum)` pair.

use lyon::{
    path::Path,
    tessellation::{
        geometry_builder::MaxIndex, BuffersBuilder, FillOptions, FillRule, FillTessellator,
        FillVertex, FillVertexConstructor, VertexBuffers, VertexId,
    },
};

use crate::{
    render::ShaderVertex,
    style::layer::{ElevationDatum, ExtrusionPart, FillExtrusionPaint, StyleProperty},
};

mod clip;
pub(super) use clip::clipped;
use clip::{is_hole, normalized_rings, on_clipped_edge, rings, signed_area};

/// Normal marker distinguishing a bottom floor-cap vertex (`0.1 < |normal| < 0.5`) from a roof
/// (`|normal| == 0.0`) or wall (`|normal| >= 1.0`).
pub const FLOOR_NORMAL: [f32; 2] = [0.25, 0.0];

/// The range of [`next_zoom_code`]'s rise, centred on zero, which one overscale step adds.
pub const NEXT_ZOOM_CODE_SPAN: f32 = 65536.0;

/// Maximum overscale steps stored below [`DATUM_CODE_SPAN`] so all 9 datum pairs stay exact in f32.
pub const MAX_PACKED_OVERSCALE: u8 = 15;

/// Stride in `corner_code` per `(base_datum, height_datum)` pair (`16 * NEXT_ZOOM_CODE_SPAN`).
pub const DATUM_CODE_SPAN: f32 = 16.0 * NEXT_ZOOM_CODE_SPAN;

/// The paint properties that shape an extrusion.
#[derive(Clone)]
pub struct ExtrusionOptions {
    /// Roof height in metres.
    pub height: Option<StyleProperty<f32>>,
    /// Wall base in metres.
    pub base: Option<StyleProperty<f32>>,
    /// Vertical datum of `base`.
    pub base_datum: Option<StyleProperty<String>>,
    /// Vertical datum of `height`.
    pub height_datum: Option<StyleProperty<String>>,
    /// Shell part selector (`"full"`, `"walls"`, `"roof"`, or `"floor"`).
    pub part: Option<StyleProperty<String>>,
    /// Closed translucent 3D volume mode: clips to `[0, 4096]`, omits tile-seam walls, and adds
    /// a bottom floor cap when `base > 0`.
    pub translucent: bool,
    /// Tile units past the tile's edge to which the polygons are clipped, for geometry that is
    /// not clipped already; translucent volumes default to `0.0` so tiles partition cleanly.
    pub clip_buffer: Option<f32>,
    /// Zoom levels the tile is drawn past its own: the zoom the heights are evaluated at less
    /// the zoom of the tile's coordinates.
    pub overscale: u8,
}

impl ExtrusionOptions {
    /// Takes the height, base, datum and translucency properties of a layer's paint, for a tile
    /// magnified `overscaling` times past its own zoom (one when it is drawn at its own zoom).
    pub fn for_paint(paint: &FillExtrusionPaint, overscaling: f64) -> Self {
        let translucent = paint.fill_extrusion_translucent.unwrap_or(false);
        Self {
            height: paint.fill_extrusion_height.clone(),
            base: paint.fill_extrusion_base.clone(),
            base_datum: paint.fill_extrusion_base_datum.clone(),
            height_datum: paint.fill_extrusion_height_datum.clone(),
            part: paint.fill_extrusion_part.clone(),
            translucent,
            clip_buffer: translucent.then_some(0.0),
            overscale: overscaling.log2().round().clamp(0.0, f64::from(u8::MAX)) as u8,
        }
    }
}

/// Base, roof height and vertical datums of a feature at the tile's zoom and one zoom above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtrusionHeights {
    /// Base at the tile's zoom.
    pub base: f32,
    /// Roof height at the tile's zoom.
    pub height: f32,
    /// Base one zoom above.
    pub base_next: f32,
    /// Roof height one zoom above.
    pub height_next: f32,
    /// Zoom levels the tile is drawn past its own, see [`ExtrusionOptions::overscale`].
    pub overscale: u8,
    /// Vertical datum of `base`.
    pub base_datum: ElevationDatum,
    /// Vertical datum of `height`.
    pub height_datum: ElevationDatum,
    /// Shell part emitted for this feature.
    pub part: ExtrusionPart,
    /// Whether this extrusion is a closed translucent 3D volume.
    pub translucent: bool,
    /// Whether walls lying along `0` or `4096` are suppressed as tile-partition seams.
    pub suppress_tile_boundary_walls: bool,
}

impl ExtrusionHeights {
    /// Evaluates a feature's heights and datums at the two zoom stops of a two-stop zoom ramp,
    /// or else at `zoom` and `zoom + 1.0`.
    pub fn for_feature(
        options: &ExtrusionOptions,
        properties: &crate::style::expression::FeatureProperties,
        zoom: f64,
    ) -> Self {
        let evaluate = |property: &Option<StyleProperty<f32>>, z| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(properties, z))
                .unwrap_or(0.0)
                .max(0.0)
        };
        let evaluate_datum = |property: &Option<StyleProperty<String>>| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(properties, zoom))
                .map_or(ElevationDatum::CentroidAgl, |value| {
                    ElevationDatum::from_extrusion_str(&value)
                })
        };
        let part = options
            .part
            .as_ref()
            .and_then(|property| property.evaluate_for(properties, zoom))
            .map_or(ExtrusionPart::Full, |value| {
                ExtrusionPart::from_extrusion_str(&value)
            });
        let (hz0, hz1) = options
            .height
            .as_ref()
            .and_then(|p| p.zoom_interpolation_bounds())
            .unwrap_or((zoom, zoom + 1.0));
        let (bz0, bz1) = options
            .base
            .as_ref()
            .and_then(|p| p.zoom_interpolation_bounds())
            .unwrap_or((zoom, zoom + 1.0));
        Self {
            height: evaluate(&options.height, hz0),
            base: evaluate(&options.base, bz0),
            height_next: evaluate(&options.height, hz1),
            base_next: evaluate(&options.base, bz1),
            overscale: options.overscale,
            base_datum: evaluate_datum(&options.base_datum),
            height_datum: evaluate_datum(&options.height_datum),
            part,
            translucent: options.translucent,
            suppress_tile_boundary_walls: options.clip_buffer == Some(0.0),
        }
    }

    /// Heights that do not change with zoom, on a tile drawn at its own zoom.
    #[cfg(test)]
    pub fn constant(base: f32, height: f32) -> Self {
        Self {
            base,
            height,
            base_next: base,
            height_next: height,
            overscale: 0,
            base_datum: ElevationDatum::CentroidAgl,
            height_datum: ElevationDatum::CentroidAgl,
            part: ExtrusionPart::Full,
            translucent: false,
            suppress_tile_boundary_walls: false,
        }
    }

    fn has_floor(self) -> bool {
        self.translucent && (self.base > 0.0 || self.base_next > 0.0)
    }

    fn apply(&self, vertex: &mut ShaderVertex, top: bool) {
        vertex.distance = self.base;
        vertex.elevation = self.height;
        let rise = if top {
            self.height_next - self.height
        } else {
            self.base_next - self.base
        };
        let pair = ElevationDatum::pack_pair(self.base_datum, self.height_datum);
        vertex.corner_code = pack_extrusion_code(rise, self.overscale, pair);
    }
}

/// Packs how many metres a vertex rises one zoom above the tile's with the tile's overscale
/// into one float: `overscale * SPAN + rise + SPAN / 2`, the rise kept within half the span.
pub fn next_zoom_code(rise: f32, overscale: u8) -> f32 {
    let half = NEXT_ZOOM_CODE_SPAN / 2.0;
    let clamped_overscale = overscale.min(MAX_PACKED_OVERSCALE);
    f32::from(clamped_overscale) * NEXT_ZOOM_CODE_SPAN + rise.clamp(-half, half - 1.0) + half
}

/// Packs `(base_datum, height_datum)` above `next_zoom_code`; `(CentroidAgl, CentroidAgl)` adds 0.
pub fn pack_extrusion_code(rise: f32, overscale: u8, datum_pair: u8) -> f32 {
    f32::from(datum_pair) * DATUM_CODE_SPAN + next_zoom_code(rise, overscale)
}

/// Unpacks `(base_datum, height_datum)` from an extrusion vertex's `corner_code`.
pub fn unpack_datum_pair(corner_code: f32) -> (ElevationDatum, ElevationDatum) {
    let pair = (corner_code / DATUM_CODE_SPAN).floor().clamp(0.0, 8.0) as u8;
    ElevationDatum::unpack_pair(pair)
}

struct RoofVertex {
    heights: ExtrusionHeights,
}

impl FillVertexConstructor<ShaderVertex> for RoofVertex {
    fn new_vertex(&mut self, vertex: FillVertex) -> ShaderVertex {
        let mut output = ShaderVertex::new(vertex.position().to_array(), [0.0, 0.0]);
        self.heights.apply(&mut output, true);
        output
    }
}

/// The longest distance along a ring in tile units before it starts over, as GL JS stores it.
const MAX_EDGE_DISTANCE: f32 = 32768.0 / 2.0;

fn push_wall<I: From<VertexId>>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    [a, b]: [[f32; 2]; 2],
    normal: [f32; 2],
    heights: ExtrusionHeights,
    edge_distances: [f32; 2],
) {
    let first = buffer.vertices.len() as u32;
    let index = |offset: u32| I::from(VertexId(first + offset));
    for (point, top, distance) in [
        (a, false, edge_distances[0]),
        (a, true, edge_distances[0]),
        (b, true, edge_distances[1]),
        (b, false, edge_distances[1]),
    ] {
        let scale = if top { 2.0 } else { 1.0 };
        let mut vertex = ShaderVertex::new(point, [normal[0] * scale, normal[1] * scale]);
        heights.apply(&mut vertex, top);
        vertex.edge_distance = distance;
        buffer.vertices.push(vertex);
    }
    buffer.indices.extend([0, 1, 2, 0, 2, 3].map(index));
}

/// Appends the roof, optional floor cap, and walls of the polygons in `path` to `buffer`.
#[cfg(test)]
pub(super) fn extrude<I>(
    path: &Path,
    heights: ExtrusionHeights,
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    (tolerance, refine_roof): (
        f32,
        impl FnOnce(&mut VertexBuffers<ShaderVertex, I>, usize) -> Result<(), String>,
    ),
) -> Result<(), String>
where
    I: From<VertexId> + std::ops::Add + MaxIndex + Copy + Into<u32>,
{
    let mut tessellator = FillTessellator::new();
    let mut earcut = super::earcut_fill::PolygonEarcut::default();
    extrude_with_tessellator(
        (&mut tessellator, &mut earcut),
        path,
        heights,
        buffer,
        (tolerance, refine_roof),
    )
}

pub(super) fn extrude_with_tessellator<I>(
    (tessellator, earcut): (&mut FillTessellator, &mut super::earcut_fill::PolygonEarcut),
    path: &Path,
    heights: ExtrusionHeights,
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    (tolerance, refine_roof): (
        f32,
        impl FnOnce(&mut VertexBuffers<ShaderVertex, I>, usize) -> Result<(), String>,
    ),
) -> Result<(), String>
where
    I: From<VertexId> + std::ops::Add + MaxIndex + Copy + Into<u32>,
{
    if heights.part != ExtrusionPart::Walls {
        let cap_vertex_start = buffer.vertices.len();
        let cap_index_start = buffer.indices.len();
        if !earcut.try_triangulate(path, true, buffer, |pos| {
            let mut output = ShaderVertex::new(pos, [0.0, 0.0]);
            heights.apply(&mut output, true);
            output
        }) {
            tessellator
                .tessellate_path(
                    path,
                    &FillOptions::tolerance(tolerance).with_fill_rule(FillRule::EvenOdd),
                    &mut BuffersBuilder::new(buffer, RoofVertex { heights }),
                )
                .map_err(|error| error.to_string())?;
        }
        let after_cap_vertices = buffer.vertices.len();
        refine_roof(buffer, cap_index_start)?;
        if heights.part == ExtrusionPart::Floor {
            for vertex in &mut buffer.vertices[cap_vertex_start..] {
                vertex.normal = FLOOR_NORMAL;
                heights.apply(vertex, false);
            }
            orient_cap_triangles(buffer, cap_index_start..buffer.indices.len(), false);
            return Ok(());
        }
        for vertex in &mut buffer.vertices[after_cap_vertices..] {
            heights.apply(vertex, true);
        }
        orient_cap_triangles(buffer, cap_index_start..buffer.indices.len(), true);
        if heights.part == ExtrusionPart::Roof {
            return Ok(());
        }
        if heights.has_floor() {
            append_floor_from_roof(
                buffer,
                cap_vertex_start..buffer.vertices.len(),
                cap_index_start..buffer.indices.len(),
                heights,
            );
        }
    }

    let raw_rings = rings(path);
    let wound_rings;
    let active_rings = if heights.translucent {
        wound_rings = normalized_rings(&raw_rings);
        &wound_rings
    } else {
        &raw_rings
    };
    for (index, ring) in active_rings.iter().enumerate() {
        let area = signed_area(ring);
        if area == 0.0 {
            continue;
        }
        let facing = if is_hole(index, active_rings) {
            area
        } else {
            -area
        }
        .signum();
        let subdivide_agl_wall = heights.base_datum == ElevationDatum::VertexAgl
            || heights.height_datum == ElevationDatum::VertexAgl;
        let mut travelled = 0.0;
        for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            if on_clipped_edge(*a, *b, heights.suppress_tile_boundary_walls) || length == 0.0 {
                continue;
            }
            if travelled + length > MAX_EDGE_DISTANCE {
                travelled = 0.0;
            }
            let start = travelled;
            travelled += length;
            let normal = [facing * dy / length, -facing * dx / length];
            let steps = if subdivide_agl_wall {
                (length / 128.0).ceil().max(1.0) as u32
            } else {
                1
            };
            for step in 0..steps {
                let t0 = (step as f32) / (steps as f32);
                let t1 = ((step + 1) as f32) / (steps as f32);
                let p0 = [a[0] + dx * t0, a[1] + dy * t0];
                let p1 = if step + 1 == steps {
                    *b
                } else {
                    [a[0] + dx * t1, a[1] + dy * t1]
                };
                let d0 = start + length * (1.0 - t0);
                let d1 = if step + 1 == steps {
                    start
                } else {
                    start + length * (1.0 - t1)
                };
                push_wall(buffer, [p0, p1], normal, heights, [d0, d1]);
            }
        }
    }
    Ok(())
}

/// Ensures every triangle in `range` winds counterclockwise when viewed from outside (`+Up` for
/// the roof where `east × north` corresponds to `cross_xy < 0`, `-Up` for the floor).
fn orient_cap_triangles<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    range: std::ops::Range<usize>,
    upward: bool,
) where
    I: Copy + Into<u32>,
{
    for tri in range.step_by(3) {
        if tri + 2 >= buffer.indices.len() {
            break;
        }
        let p0 = buffer.vertices[buffer.indices[tri].into() as usize].position;
        let p1 = buffer.vertices[buffer.indices[tri + 1].into() as usize].position;
        let p2 = buffer.vertices[buffer.indices[tri + 2].into() as usize].position;
        let cross_xy = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
        if (upward && cross_xy > 0.0) || (!upward && cross_xy < 0.0) {
            buffer.indices.swap(tri + 1, tri + 2);
        }
    }
}

fn append_floor_from_roof<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    roof_vertices: std::ops::Range<usize>,
    roof_indices: std::ops::Range<usize>,
    heights: ExtrusionHeights,
) where
    I: From<VertexId> + Copy + Into<u32>,
{
    let floor_vertex_base = buffer.vertices.len();
    buffer.vertices.reserve(roof_vertices.len());
    for index in roof_vertices.clone() {
        let mut vertex = buffer.vertices[index];
        vertex.normal = FLOOR_NORMAL;
        heights.apply(&mut vertex, false);
        buffer.vertices.push(vertex);
    }
    buffer.indices.reserve(roof_indices.len());
    let remap = |idx: I| -> I {
        let orig = idx.into() as usize;
        I::from(VertexId::from_usize(
            floor_vertex_base + (orig - roof_vertices.start),
        ))
    };
    for tri in roof_indices.step_by(3) {
        if tri + 2 >= buffer.indices.len() {
            break;
        }
        let i0 = remap(buffer.indices[tri]);
        let i1 = remap(buffer.indices[tri + 1]);
        let i2 = remap(buffer.indices[tri + 2]);
        // Roof triangles wind +Up (`cross_xy < 0`); swapping `i1` and `i2` winds the floor -Up.
        buffer.indices.extend([i0, i2, i1]);
    }
}

#[cfg(test)]
mod tests;

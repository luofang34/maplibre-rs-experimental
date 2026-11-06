//! Walls and roofs of extruded polygons.
//!
//! A vertex carries its place in the fields the fill and line shaders use otherwise: the
//! wall's lighting normal in `normal`, scaled to length two on the top edge so the shader can
//! tell it from the bottom edge, the base in `distance`, and the roof height in `elevation`.
//! A roof vertex has the zero normal. `corner_code` packs, as [`next_zoom_code`] does, how much
//! the vertex rises one zoom above the tile's, which the shader blends in by the fractional
//! zoom; being relative, it survives the ground elevation added to the heights later.

use lyon::{
    path::{Event, Path},
    tessellation::{
        geometry_builder::MaxIndex, BuffersBuilder, FillOptions, FillRule, FillTessellator,
        FillVertex, FillVertexConstructor, VertexBuffers, VertexId,
    },
};

use crate::{
    coords::EXTENT,
    render::ShaderVertex,
    style::layer::{FillExtrusionPaint, StyleProperty},
};

/// The paint properties that shape an extrusion.
#[derive(Clone)]
pub struct ExtrusionOptions {
    /// Roof height in metres.
    pub height: Option<StyleProperty<f32>>,
    /// Wall base in metres.
    pub base: Option<StyleProperty<f32>>,
    /// Tile units past the tile's edge to which the polygons are clipped, for geometry that is
    /// not clipped already; a vector tile's polygons are.
    pub clip_buffer: Option<f32>,
    /// Zoom levels the tile is drawn past its own: the zoom the heights are evaluated at less
    /// the zoom of the tile's coordinates.
    pub overscale: u8,
}

impl ExtrusionOptions {
    /// Takes the height and base properties of a layer's paint.
    pub fn for_paint(paint: &FillExtrusionPaint) -> Self {
        Self {
            height: paint.fill_extrusion_height.clone(),
            base: paint.fill_extrusion_base.clone(),
            clip_buffer: None,
            overscale: 0,
        }
    }
}

/// Base and roof height in metres at the zoom a tile is evaluated at and at the zoom above.
///
/// GL JS evaluates a paint property that depends on both zoom and feature at those two zooms
/// and interpolates between them by the fractional zoom, so a building whose height ramps in
/// from zoom 15 to 16 stands at half its height at zoom 15.5.
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
}

impl ExtrusionHeights {
    /// Heights that do not change with zoom, on a tile drawn at its own zoom.
    #[cfg(test)]
    pub fn constant(base: f32, height: f32) -> Self {
        Self {
            base,
            height,
            base_next: base,
            height_next: height,
            overscale: 0,
        }
    }

    fn apply(&self, vertex: &mut ShaderVertex, top: bool) {
        vertex.distance = self.base;
        vertex.elevation = self.height;
        let rise = if top {
            self.height_next - self.height
        } else {
            self.base_next - self.base
        };
        vertex.corner_code = next_zoom_code(rise, self.overscale);
    }
}

/// The range of [`next_zoom_code`]'s rise, centred on zero, which one overscale step adds.
pub const NEXT_ZOOM_CODE_SPAN: f32 = 65536.0;

/// Packs how many metres a vertex rises one zoom above the tile's with the tile's overscale
/// into one float: `overscale * SPAN + rise + SPAN / 2`, the rise kept within half the span.
/// The shader needs both to blend by the zoom past the one the heights were evaluated at, and
/// its vertex attributes are all taken.
pub fn next_zoom_code(rise: f32, overscale: u8) -> f32 {
    let half = NEXT_ZOOM_CODE_SPAN / 2.0;
    f32::from(overscale) * NEXT_ZOOM_CODE_SPAN + rise.clamp(-half, half - 1.0) + half
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

type Ring = Vec<[f32; 2]>;

fn rings(path: &Path) -> Vec<Ring> {
    let mut rings = Vec::new();
    let mut current = Ring::new();
    for event in path.iter() {
        match event {
            Event::Begin { at } => current = vec![at.to_array()],
            Event::Line { to, .. } => current.push(to.to_array()),
            Event::End { .. } => rings.push(std::mem::take(&mut current)),
            Event::Quadratic { to, .. } | Event::Cubic { to, .. } => current.push(to.to_array()),
        }
    }
    rings
}

/// The sum of `(x2 - x1) * (y1 + y2)` over the ring's edges, as geojson-vt measures the winding.
fn trapezoid_area(ring: &Ring) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| (b[0] - a[0]) * (a[1] + b[1]))
        .sum()
}

/// Twice the signed area in the raw coordinates; the sign tells which side the inside is on.
fn signed_area(ring: &Ring) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum()
}

fn contains(ring: &Ring, [x, y]: [f32; 2]) -> bool {
    let mut inside = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// A ring inside an odd number of others is a hole.
fn is_hole(index: usize, rings: &[Ring]) -> bool {
    let Some(&point) = rings[index].first() else {
        return false;
    };
    rings
        .iter()
        .enumerate()
        .filter(|(other, ring)| *other != index && contains(ring, point))
        .count()
        % 2
        == 1
}

/// Walls along a tile's clipped edge would show a seam between tiles.
fn on_clipped_edge(a: [f32; 2], b: [f32; 2]) -> bool {
    let extent = EXTENT as f32;
    (a[0] == b[0] && (a[0] < 0.0 || a[0] > extent))
        || (a[1] == b[1] && (a[1] < 0.0 || a[1] > extent))
}

/// `ring` cut to the square from `low` to `high` on both axes, one edge of the square at a time.
fn clip_ring(ring: &Ring, (low, high): (f32, f32)) -> Ring {
    let mut clipped = ring.clone();
    for axis in 0..2 {
        for keep_above in [true, false] {
            let bound = if keep_above { low } else { high };
            let inside = |point: &[f32; 2]| {
                if keep_above {
                    point[axis] >= bound
                } else {
                    point[axis] <= bound
                }
            };
            let mut next = Ring::new();
            for (a, b) in clipped.iter().zip(clipped.iter().cycle().skip(1)) {
                if inside(a) {
                    next.push(*a);
                }
                if inside(a) != inside(b) {
                    let t = (bound - a[axis]) / (b[axis] - a[axis]);
                    let mut crossing = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                    crossing[axis] = bound;
                    next.push(crossing);
                }
            }
            clipped = next;
        }
    }
    clipped
}

/// `path` with each ring clipped to `buffer` tile units past the tile and wound the way a
/// GeoJSON source winds them, outer rings counterclockwise on the map and holes the other way;
/// a ring left with no area is dropped. The wall distance of a pattern runs along that winding.
pub(super) fn clipped(path: &Path, buffer: f32) -> Path {
    let bounds = (-buffer, EXTENT as f32 + buffer);
    let rings: Vec<Ring> = rings(path)
        .iter()
        .map(|ring| clip_ring(ring, bounds))
        .filter(|ring| ring.len() >= 3 && signed_area(ring) != 0.0)
        .collect();
    let mut builder = Path::builder();
    for (index, ring) in rings.iter().enumerate() {
        let mut ring = ring.clone();
        // An outer ring keeps its order when its area is not positive, a hole when it is, as
        // geojson-vt winds them.
        if (trapezoid_area(&ring) > 0.0) != is_hole(index, &rings) {
            ring[1..].reverse();
        }
        builder.begin(lyon::math::point(ring[0][0], ring[0][1]));
        for point in &ring[1..] {
            builder.line_to(lyon::math::point(point[0], point[1]));
        }
        builder.end(true);
    }
    builder.build()
}

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

/// Appends the roof and walls of the polygons in `path` to `buffer`.
///
/// `refine_roof` may split the roof's triangles from the given index on, as a globe tile does so
/// the roof follows the sphere; the vertices it adds take the roof's base and height.
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
    I: From<VertexId> + std::ops::Add + MaxIndex,
{
    let roof_start = buffer.indices.len();
    let vertex_start = buffer.vertices.len();
    FillTessellator::new()
        .tessellate_path(
            path,
            &FillOptions::tolerance(tolerance).with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(buffer, RoofVertex { heights }),
        )
        .map_err(|error| error.to_string())?;
    refine_roof(buffer, roof_start)?;
    for vertex in &mut buffer.vertices[vertex_start..] {
        heights.apply(vertex, true);
    }
    let rings = rings(path);
    for (index, ring) in rings.iter().enumerate() {
        let area = signed_area(ring);
        if area == 0.0 {
            continue;
        }
        // The inside of the ring lies on the side `area` points to. GL JS lights a wall by the
        // normal pointing into the extrusion, so the light meets the walls it shines on the
        // other way round: the wall of an outer ring takes the inside direction and the wall
        // of a hole the direction into the ring's outside.
        let facing = if is_hole(index, &rings) { area } else { -area }.signum();
        let mut travelled = 0.0;
        for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            // The distance along the ring counts only the walls drawn, and starts over before it
            // outgrows what GL JS stores.
            if on_clipped_edge(*a, *b) {
                continue;
            }
            if travelled + length > MAX_EDGE_DISTANCE {
                travelled = 0.0;
            }
            let start = travelled;
            travelled += length;
            if length == 0.0 {
                continue;
            }
            let normal = [facing * dy / length, -facing * dx / length];
            // GL JS gives the end of an edge the distance before it and the start the distance
            // after it.
            push_wall(buffer, [*a, *b], normal, heights, [travelled, start]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

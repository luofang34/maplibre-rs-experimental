//! Walls and roofs of extruded polygons.
//!
//! A vertex carries its place in the fields the fill and line shaders use otherwise: the
//! wall's lighting normal in `normal`, scaled to length two on the top edge so the shader can
//! tell it from the bottom edge, the base in `distance`, and the roof height in `elevation`.
//! A roof vertex has the zero normal.

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
}

impl ExtrusionOptions {
    /// Takes the height and base properties of a layer's paint.
    pub fn for_paint(paint: &FillExtrusionPaint) -> Self {
        Self {
            height: paint.fill_extrusion_height.clone(),
            base: paint.fill_extrusion_base.clone(),
        }
    }
}

struct RoofVertex {
    base: f32,
    height: f32,
}

impl FillVertexConstructor<ShaderVertex> for RoofVertex {
    fn new_vertex(&mut self, vertex: FillVertex) -> ShaderVertex {
        let mut output = ShaderVertex::new(vertex.position().to_array(), [0.0, 0.0]);
        output.distance = self.base;
        output.elevation = self.height;
        output
    }
}

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

fn push_wall<I: From<VertexId>>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    [a, b]: [[f32; 2]; 2],
    normal: [f32; 2],
    (base, height): (f32, f32),
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
        vertex.distance = base;
        vertex.elevation = height;
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
    heights: (f32, f32),
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    (tolerance, refine_roof): (
        f32,
        impl FnOnce(&mut VertexBuffers<ShaderVertex, I>, usize) -> Result<(), String>,
    ),
) -> Result<(), String>
where
    I: From<VertexId> + std::ops::Add + MaxIndex,
{
    let (base, height) = heights;
    let roof_start = buffer.indices.len();
    let vertex_start = buffer.vertices.len();
    FillTessellator::new()
        .tessellate_path(
            path,
            &FillOptions::tolerance(tolerance).with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(buffer, RoofVertex { base, height }),
        )
        .map_err(|error| error.to_string())?;
    refine_roof(buffer, roof_start)?;
    for vertex in &mut buffer.vertices[vertex_start..] {
        vertex.distance = base;
        vertex.elevation = height;
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
            let start = travelled;
            travelled += length;
            if length == 0.0 || on_clipped_edge(*a, *b) {
                continue;
            }
            let normal = [facing * dy / length, -facing * dx / length];
            push_wall(buffer, [*a, *b], normal, heights, [start, travelled]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use super::*;

pub(super) fn extrude_caps<I>(
    (tessellator, earcut): (
        &mut FillTessellator,
        &mut super::super::earcut_fill::PolygonEarcut,
    ),
    active_path: &Path,
    heights: ExtrusionHeights,
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    (tolerance, refine_roof): (
        f32,
        impl FnOnce(&mut VertexBuffers<ShaderVertex, I>, usize) -> Result<(), String>,
    ),
) -> Result<(bool, bool), String>
where
    I: From<VertexId> + std::ops::Add + MaxIndex + Copy + Into<u32>,
{
    let cap_vertex_start = buffer.vertices.len();
    let cap_index_start = buffer.indices.len();
    let earcut_used = earcut.try_triangulate(active_path, true, buffer, |pos| {
        let mut output = ShaderVertex::new(pos, [0.0, 0.0]);
        heights.apply(&mut output, true);
        output
    });
    if !earcut_used {
        tessellator
            .tessellate_path(
                active_path,
                &FillOptions::tolerance(tolerance).with_fill_rule(FillRule::EvenOdd),
                &mut BuffersBuilder::new(buffer, RoofVertex { heights }),
            )
            .map_err(|error| error.to_string())?;
    }
    let after_cap_vertices = buffer.vertices.len();
    refine_roof(buffer, cap_index_start)?;
    if heights.part == ExtrusionPart::Floor {
        if let Some(verts) = buffer.vertices.get_mut(cap_vertex_start..) {
            for vertex in verts {
                vertex.normal = FLOOR_NORMAL;
                heights.apply(vertex, false);
            }
        }
        orient_cap_triangles(buffer, cap_index_start..buffer.indices.len(), false);
        return Ok((earcut_used, true));
    }
    if let Some(verts) = buffer.vertices.get_mut(after_cap_vertices..) {
        for vertex in verts {
            heights.apply(vertex, true);
        }
    }
    orient_cap_triangles(buffer, cap_index_start..buffer.indices.len(), true);
    if heights.part == ExtrusionPart::Roof {
        return Ok((earcut_used, true));
    }
    if heights.has_floor() {
        append_floor_from_roof(
            buffer,
            cap_vertex_start..buffer.vertices.len(),
            cap_index_start..buffer.indices.len(),
            heights,
        );
    }
    Ok((earcut_used, false))
}

pub(super) fn extrude_walls<I>(
    active_rings: &[Vec<[f32; 2]>],
    heights: ExtrusionHeights,
    buffer: &mut VertexBuffers<ShaderVertex, I>,
) where
    I: From<VertexId> + std::ops::Add + MaxIndex + Copy + Into<u32>,
{
    let subdivide_agl_wall = heights.base_datum == ElevationDatum::VertexAgl
        || heights.height_datum == ElevationDatum::VertexAgl;
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
            push_wall_steps(
                buffer,
                (*a, *b),
                (dx, dy, length, start),
                normal,
                heights,
                steps,
            );
        }
    }
}

fn push_wall_steps<I>(
    buffer: &mut VertexBuffers<ShaderVertex, I>,
    (a, b): ([f32; 2], [f32; 2]),
    (dx, dy, length, start): (f32, f32, f32, f32),
    normal: [f32; 2],
    heights: ExtrusionHeights,
    steps: u32,
) where
    I: From<VertexId> + std::ops::Add + MaxIndex + Copy + Into<u32>,
{
    for step in 0..steps {
        let t0 = (step as f32) / (steps as f32);
        let t1 = ((step + 1) as f32) / (steps as f32);
        let p0 = [a[0] + dx * t0, a[1] + dy * t0];
        let p1 = if step + 1 == steps {
            b
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
        let (Some(&i0), Some(&i1), Some(&i2)) = (
            buffer.indices.get(tri),
            buffer.indices.get(tri + 1),
            buffer.indices.get(tri + 2),
        ) else {
            break;
        };
        let (Some(v0), Some(v1), Some(v2)) = (
            buffer.vertices.get(i0.into() as usize),
            buffer.vertices.get(i1.into() as usize),
            buffer.vertices.get(i2.into() as usize),
        ) else {
            continue;
        };
        let (p0, p1, p2) = (v0.position, v1.position, v2.position);
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
        let Some(mut vertex) = buffer.vertices.get(index).copied() else {
            continue;
        };
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
        let (Some(&i0), Some(&i1), Some(&i2)) = (
            buffer.indices.get(tri),
            buffer.indices.get(tri + 1),
            buffer.indices.get(tri + 2),
        ) else {
            break;
        };
        // Roof triangles wind +Up (`cross_xy < 0`); swapping `i1` and `i2` winds the floor -Up.
        buffer.indices.extend([remap(i0), remap(i2), remap(i1)]);
    }
}

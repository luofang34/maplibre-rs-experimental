#![allow(clippy::expect_used, clippy::panic)]

use super::{create_terrain_mesh, TERRAIN_MESH_SIZE};
use crate::coords::EXTENT_SINT;

#[test]
fn grid_and_skirts_have_the_expected_sizes() {
    let n = TERRAIN_MESH_SIZE;
    let mesh = create_terrain_mesh(n);

    let grid = (n + 1) * (n + 1);
    let skirts = 2 * (n + 1) + 4 * (n + 1);
    assert_eq!(mesh.vertices.len() as u32, grid + skirts + 2 * (n + 1));
    assert_eq!(
        mesh.indices.len() as u32,
        n * n * 6 + n * 12 + n * 12 + n * 12
    );
    assert_eq!(mesh.indices.len() % 3, 0);
}

#[test]
fn indices_stay_in_range_and_skirts_hang_from_the_edges() {
    let mesh = create_terrain_mesh(4);
    let count = mesh.vertices.len() as u32;

    assert!(mesh.indices.iter().all(|index| *index < count));
    let skirt_vertices: Vec<_> = mesh.vertices.iter().filter(|v| v.skirt == 1).collect();
    assert_eq!(skirt_vertices.len(), 2 * 5 + 2 * 5);
    assert!(skirt_vertices.iter().all(|v| {
        v.x == 0 || v.y == 0 || i32::from(v.x) == EXTENT_SINT || i32::from(v.y) == EXTENT_SINT
    }));
    let grid: Vec<_> = mesh.vertices.iter().take(25).collect();
    assert!(grid.iter().all(|v| v.skirt == 0));
    assert_eq!(
        (grid[24].x, grid[24].y),
        (EXTENT_SINT as i16, EXTENT_SINT as i16)
    );
}

#[test]
fn terrain_grid_and_polar_fans_face_outward() {
    use cgmath::{InnerSpace, Vector3};
    let mesh = create_terrain_mesh(TERRAIN_MESH_SIZE);
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let vertices: [_; 3] = std::array::from_fn(|i| mesh.vertices[triangle[i] as usize]);
        if vertices.iter().any(|vertex| vertex.skirt != 0) {
            continue;
        }
        let poles: Vec<_> = vertices
            .iter()
            .filter(|v| matches!(v.y, i16::MIN | i16::MAX))
            .collect();
        if poles.len() == 2 && poles[0].y == poles[1].y {
            continue;
        }
        let [a, b, c] = vertices.map(|vertex| match vertex.y {
            i16::MIN => Vector3::unit_y(),
            i16::MAX => -Vector3::unit_y(),
            _ => crate::projection::globe::project_tile_coordinates_to_unit_sphere(
                0,
                0,
                0,
                f64::from(vertex.x),
                f64::from(vertex.y),
            ),
        });
        let outward = (b - a).cross(c - a).dot(a + b + c);
        assert!(
            outward > 0.0,
            "surface triangle {triangle:?} faces inward ({outward})"
        );
    }
}

#[test]
fn terrain_skirts_face_outside_each_tile_edge() {
    use cgmath::{InnerSpace, Vector3};
    let mesh = create_terrain_mesh(4);
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let vertices: [_; 3] = std::array::from_fn(|i| mesh.vertices[triangle[i] as usize]);
        if !vertices.iter().any(|vertex| vertex.skirt != 0) {
            continue;
        }
        let [a, b, c] = vertices.map(|vertex| {
            Vector3::new(
                f64::from(vertex.x),
                -f64::from(vertex.y),
                -f64::from(vertex.skirt),
            )
        });
        let outward = (a + b + c) / 3.0
            - Vector3::new(
                f64::from(EXTENT_SINT) / 2.0,
                -f64::from(EXTENT_SINT) / 2.0,
                0.0,
            );
        assert!(
            (b - a).cross(c - a).dot(outward) > 0.0,
            "skirt triangle {triangle:?} faces into tile"
        );
    }
}

#[test]
fn polar_caps_cover_their_interpolated_strips() {
    let mesh = create_terrain_mesh(TERRAIN_MESH_SIZE);
    let mut twice_area = 0_i64;
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let vertices: [_; 3] = std::array::from_fn(|i| mesh.vertices[triangle[i] as usize]);
        if !vertices.iter().any(|v| matches!(v.y, i16::MIN | i16::MAX)) {
            continue;
        }
        let [a, b, c] = vertices.map(|v| [i64::from(v.x), i64::from(v.y)]);
        let signed = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(
            signed < 0,
            "cap triangle must face above the Mercator plane"
        );
        twice_area -= signed;
    }
    let extent = i64::from(EXTENT_SINT);
    let north = -i64::from(i16::MIN);
    let south = i64::from(i16::MAX) - extent;
    assert_eq!(
        twice_area,
        2 * extent * (north + south),
        "separated pole markers require complete strips during projection blending"
    );
}

#[test]
fn three_cell_grid_covers_tile_and_connects_its_skirts() {
    assert_tile_coverage_and_skirts(3);
}

#[test]
fn five_cell_grid_covers_tile_and_connects_its_skirts() {
    assert_tile_coverage_and_skirts(5);
}

#[test]
fn odd_fine_grid_covers_tile_and_connects_its_skirts() {
    assert_tile_coverage_and_skirts(127);
}

#[test]
fn zero_resolution_uses_one_cell() {
    assert_eq!(create_terrain_mesh(0), create_terrain_mesh(1));
}

fn assert_tile_coverage_and_skirts(n: u32) {
    let mesh = create_terrain_mesh(n);
    let grid_count = (n + 1) * (n + 1);
    let faces: Vec<&[u32]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|face| face.as_slice())
        .filter(|face| face.iter().all(|i| *i < grid_count))
        .collect();
    let mut twice_area = 0_i64;
    for face in &faces {
        let [a, b, c]: [_; 3] = std::array::from_fn(|i| {
            let v = mesh.vertices[face[i] as usize];
            [i64::from(v.x), i64::from(v.y)]
        });
        let signed = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(
            signed < 0,
            "grid {n} contains a collapsed or inverted triangle"
        );
        twice_area -= signed;
    }
    let edge = f64::from(EXTENT_SINT);
    for point in [
        [edge - 0.25, edge * 0.25],
        [edge - 0.25, edge * 0.75],
        [edge * 0.25, edge - 0.25],
        [edge * 0.75, edge - 0.25],
        [edge - 0.25, edge - 0.25],
    ] {
        assert!(
            faces.iter().any(|face| contains_point(&mesh, face, point)),
            "grid {n} leaves {point:?} uncovered by its surface triangles"
        );
    }
    assert_eq!(
        twice_area,
        2 * i64::from(EXTENT_SINT).pow(2),
        "grid {n} does not cover the full tile surface"
    );
    assert_skirts_attach_to_boundary(&mesh, &faces, n);
}

fn contains_point(mesh: &super::TerrainMesh, face: &[u32], point: [f64; 2]) -> bool {
    let vertices: [_; 3] = std::array::from_fn(|i| {
        let v = mesh.vertices[face[i] as usize];
        [f64::from(v.x), f64::from(v.y)]
    });
    (0..3).all(|i| {
        let a = vertices[i];
        let b = vertices[(i + 1) % 3];
        (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0]) <= 0.0
    })
}

fn assert_skirts_attach_to_boundary(mesh: &super::TerrainMesh, faces: &[&[u32]], n: u32) {
    use std::collections::{BTreeSet, HashMap};
    let mut incidence = HashMap::new();
    for face in faces {
        for i in 0..3 {
            let (a, b) = (face[i], face[(i + 1) % 3]);
            let edge = (a.min(b), a.max(b));
            *incidence.entry(edge).or_insert(0_u32) += 1;
        }
    }
    let xy = |i: u32| {
        let v = mesh.vertices[i as usize];
        (v.x, v.y)
    };
    let boundary: BTreeSet<_> = incidence
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|((a, b), _)| ordered_edge(xy(a), xy(b)))
        .collect();
    let points: BTreeSet<_> = boundary.iter().flat_map(|(a, b)| [*a, *b]).collect();
    let mut attached = BTreeSet::new();
    for face in mesh.indices.as_chunks::<3>().0 {
        if !face.iter().any(|i| mesh.vertices[*i as usize].skirt == 1) {
            continue;
        }
        let ground: Vec<_> = face
            .iter()
            .filter(|i| mesh.vertices[**i as usize].skirt == 0)
            .map(|i| xy(*i))
            .collect();
        if ground.len() == 2 {
            attached.insert(ordered_edge(ground[0], ground[1]));
        }
        for i in face
            .iter()
            .filter(|i| mesh.vertices[**i as usize].skirt == 1)
        {
            assert!(
                points.contains(&xy(*i)),
                "grid {n} skirt vertex {:?} does not descend from a surface boundary vertex",
                xy(*i)
            );
        }
    }
    assert_eq!(attached, boundary, "grid {n} has an open skirt attachment");
}

fn ordered_edge(a: (i16, i16), b: (i16, i16)) -> ((i16, i16), (i16, i16)) {
    (a.min(b), a.max(b))
}

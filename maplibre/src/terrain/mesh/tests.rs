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
    for triangle in mesh.indices.chunks_exact(3) {
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
    for triangle in mesh.indices.chunks_exact(3) {
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
    for triangle in mesh.indices.chunks_exact(3) {
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

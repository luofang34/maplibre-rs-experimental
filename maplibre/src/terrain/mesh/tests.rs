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
        n * n * 6 + n * 12 + n * 12 + n * 6
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

//! Regular grid mesh with skirts, shared by every terrain tile.

use crate::coords::{EXTENT_SINT, EXTENT_UINT};

/// Grid cells per tile edge, as in GL JS `meshSize`.
pub const TERRAIN_MESH_SIZE: u32 = 128;

/// Vertex of the terrain grid in tile units with a skirt marker.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
pub struct TerrainVertex {
    /// Horizontal tile coordinate.
    pub x: i16,
    /// Vertical tile coordinate.
    pub y: i16,
    /// One for skirt vertices, which the shader drops by the skirt length.
    pub skirt: u16,
    padding: u16,
}

impl TerrainVertex {
    const fn new(x: i32, y: i32, skirt: u16) -> Self {
        Self {
            x: x as i16,
            y: y as i16,
            skirt,
            padding: 0,
        }
    }
}

/// CPU-side terrain mesh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainMesh {
    /// Grid and skirt vertices.
    pub vertices: Vec<TerrainVertex>,
    /// Triangle list indices.
    pub indices: Vec<u32>,
}

/// Builds the grid with a skirt along each edge, following GL JS `getTerrainMesh`.
///
/// Skirts hang below the tile edges so neighbouring tiles at different zoom levels do not
/// show cracks between their independently sampled edges. A zero resolution uses one cell.
pub fn create_terrain_mesh(mesh_size: u32) -> TerrainMesh {
    let n = mesh_size.max(1);
    let mut mesh = TerrainMesh::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.vertices.push(TerrainVertex::new(
                grid_coordinate(x, n),
                grid_coordinate(y, n),
                0,
            ));
        }
    }
    let row = n + 1;
    for y in 0..n {
        for x in 0..n {
            let index = y * row + x;
            mesh.indices.extend([
                index,
                index + row,
                index + row + 1,
                index,
                index + row + 1,
                index + 1,
            ]);
        }
    }
    build_skirts(&mut mesh, n);
    build_polar_caps(&mut mesh, n);
    mesh
}

fn grid_coordinate(index: u32, cells: u32) -> i32 {
    (u64::from(index) * u64::from(EXTENT_UINT) / u64::from(cells)) as i32
}

fn build_skirts(mesh: &mut TerrainMesh, n: u32) {
    let row = n + 1;
    let top = mesh.vertices.len() as u32;
    let top_edge = 0;
    let bottom = top + row;
    let bottom_edge = row * n;
    for x in 0..=n {
        mesh.vertices
            .push(TerrainVertex::new(grid_coordinate(x, n), 0, 1));
    }
    for x in 0..=n {
        mesh.vertices
            .push(TerrainVertex::new(grid_coordinate(x, n), EXTENT_SINT, 1));
    }
    for x in 0..n {
        mesh.indices.extend([
            bottom_edge + x,
            bottom + x,
            bottom + x + 1,
            bottom_edge + x,
            bottom + x + 1,
            bottom_edge + x + 1,
            top_edge + x,
            top + x + 1,
            top + x,
            top_edge + x,
            top_edge + x + 1,
            top + x + 1,
        ]);
    }
    let left = mesh.vertices.len() as u32;
    let right = left + row * 2;
    for x in [0, EXTENT_SINT] {
        for y in 0..=n {
            for skirt in [0, 1] {
                mesh.vertices
                    .push(TerrainVertex::new(x, grid_coordinate(y, n), skirt));
            }
        }
    }
    for y in (0..n * 2).step_by(2) {
        mesh.indices.extend([
            left + y,
            left + y + 1,
            left + y + 3,
            left + y,
            left + y + 3,
            left + y + 2,
            right + y,
            right + y + 3,
            right + y + 1,
            right + y,
            right + y + 2,
            right + y + 3,
        ]);
    }
}

fn build_polar_caps(mesh: &mut TerrainMesh, n: u32) {
    for (edge, marker) in [(0, i16::MIN), (n * (n + 1), i16::MAX)] {
        let start = mesh.vertices.len() as u32;
        for x in 0..=n {
            mesh.vertices.push(TerrainVertex::new(
                grid_coordinate(x, n),
                i32::from(marker),
                0,
            ));
        }
        for x in 0..n {
            // Pole vertices coincide only at a fully spherical projection.
            for mut triangle in [
                [edge + x, edge + x + 1, start + x],
                [edge + x + 1, start + x + 1, start + x],
            ] {
                if marker == i16::MAX {
                    triangle.swap(0, 1);
                }
                mesh.indices.extend(triangle);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
mod pixels;

//! Regular grid mesh with skirts, shared by every terrain tile.

use crate::coords::EXTENT_SINT;

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
/// show cracks between their independently sampled edges.
pub fn create_terrain_mesh(mesh_size: u32) -> TerrainMesh {
    let n = mesh_size.max(1);
    let delta = EXTENT_SINT / n as i32;
    let mut mesh = TerrainMesh::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.vertices
                .push(TerrainVertex::new(x as i32 * delta, y as i32 * delta, 0));
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
    build_skirts(&mut mesh, n, delta);
    mesh
}

fn build_skirts(mesh: &mut TerrainMesh, n: u32, delta: i32) {
    let row = n + 1;
    let top = mesh.vertices.len() as u32;
    let top_edge = 0;
    let bottom = top + row;
    let bottom_edge = row * n;
    for x in 0..=n {
        mesh.vertices
            .push(TerrainVertex::new(x as i32 * delta, 0, 1));
    }
    for x in 0..=n {
        mesh.vertices
            .push(TerrainVertex::new(x as i32 * delta, EXTENT_SINT, 1));
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
                    .push(TerrainVertex::new(x, y as i32 * delta, skirt));
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

#[cfg(test)]
mod tests;

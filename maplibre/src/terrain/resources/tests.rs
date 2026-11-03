use super::{
    edge_heights::{edge_row_capacity, edge_texture_size, EDGE_TEXELS_PER_TILE},
    *,
};

#[test]
fn terrain_bindings_need_no_storage_buffers_webgl2_lacks() {
    let storage: Vec<_> = TerrainResources::bind_group_layout_entries()
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.ty,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { .. },
                    ..
                } | wgpu::BindingType::StorageTexture { .. }
            )
        })
        .map(|entry| entry.binding)
        .collect();
    assert_eq!(storage, Vec::<u32>::new());
}

#[test]
fn a_tile_of_edge_heights_fills_one_row_of_the_edge_texture() {
    assert_eq!(EDGE_TEXELS_PER_TILE, TERRAIN_MESH_SIZE + 1);
    assert_eq!(edge_texture_size(3).height, 3);
    assert_eq!(edge_row_capacity(512), 512);
    assert_eq!(edge_row_capacity(4096), UNIFORM_CAPACITY as u32);
}

//! Test readback through the production terrain elevation sampling shader.
#![allow(clippy::expect_used, clippy::panic)]

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use super::DemTile;
use crate::{
    coords::EXTENT,
    render::{
        resource::Texture,
        shaders::{Shader, TerrainShader},
    },
    terrain::resources::TerrainTileUniforms,
};

const SAMPLE_COUNT: u32 = 81;
const READBACK_SIZE: u64 = SAMPLE_COUNT as u64 * 4;

pub(crate) fn pipeline(device: &wgpu::Device) -> wgpu::ComputePipeline {
    let shader = TerrainShader {
        format: wgpu::TextureFormat::Rgba8Unorm,
    };
    let source = format!(
        "{}\n{}",
        shader.describe_vertex().source,
        r#"
@group(2) @binding(0) var<storage, read_write> elevations: array<f32>;
@compute @workgroup_size(1)
fn sample_dem(@builtin(global_invocation_id) id: vec3<u32>) {
    let position = vec2<f32>(f32(id.x % 9u), f32(id.x / 9u)) * TERRAIN_EXTENT / 8.0;
    elevations[id.x] = terrain_height_gradient(position).x;
}
"#
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain DEM sampling regression"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("terrain DEM sampling regression"),
        layout: None,
        module: &module,
        entry_point: Some("sample_dem"),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn tile_bindings(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
    texture: &Texture,
) -> wgpu::BindGroup {
    let mut uniforms = TerrainTileUniforms::zeroed();
    uniforms.dem_matrix[0][0] = 1.0 / EXTENT as f32;
    uniforms.dem_matrix[1][1] = 1.0 / EXTENT as f32;
    uniforms.dem_matrix[3][3] = 1.0;
    uniforms.dem_unpack = dem.unpack().map(|value| value as f32);
    uniforms.dem_dim = dem.dim() as f32;
    uniforms.exaggeration = 1.0;
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DEM regression uniforms"),
        contents: bytemuck::bytes_of(&uniforms),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("DEM regression tile"),
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&texture.view),
            },
        ],
    })
}

pub(crate) fn sample_gpu_blocking(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
    texture: &Texture,
) -> Vec<f32> {
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("DEM sampled heights"),
        size: READBACK_SIZE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("DEM height readback"),
        size: READBACK_SIZE,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[],
    });
    let tile = tile_bindings(device, pipeline, dem, texture);
    let result = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(2),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &empty, &[]);
        pass.set_bind_group(1, &tile, &[]);
        pass.set_bind_group(2, &result, &[]);
        pass.dispatch_workgroups(SAMPLE_COUNT, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, READBACK_SIZE);
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("map readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    let data = readback
        .slice(..)
        .get_mapped_range()
        .expect("readback range");
    let heights = bytemuck::cast_slice::<u8, f32>(&data).to_vec();
    drop(data);
    readback.unmap();
    heights
}

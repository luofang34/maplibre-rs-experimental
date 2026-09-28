use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use super::{DemTile, Fixture, RgbaImage, EXTENT};
use crate::{
    render::{
        resource::Texture,
        settings::Msaa,
        shaders::{Shader, TerrainShader},
    },
    terrain::resources::TerrainTileUniforms,
};

const SAMPLE_COUNT: u32 = 81;
const READBACK_SIZE: u64 = SAMPLE_COUNT as u64 * 4;

fn pipeline(device: &wgpu::Device) -> wgpu::ComputePipeline {
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
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
) -> wgpu::BindGroup {
    let texture = Texture::new(
        Some("DEM regression"),
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        dem.stride(),
        dem.stride(),
        Msaa { samples: 1 },
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    queue.write_texture(
        texture.texture.as_image_copy(),
        dem.pixels(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(dem.stride() * 4),
            rows_per_image: None,
        },
        texture.size,
    );
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

fn sample_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
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
    let tile = tile_bindings(device, queue, pipeline, dem);
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

fn assert_gpu_matches_cpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
) {
    let heights = sample_gpu(device, queue, pipeline, dem);
    for (i, actual) in heights.into_iter().enumerate() {
        let x = (i % 9) as f64 * EXTENT / 8.0;
        let y = (i / 9) as f64 * EXTENT / 8.0;
        let expected = dem.elevation_at_tile_coords(x, y);
        let tolerance = expected.abs().max(1.0) * 1e-6;
        assert!(
            (f64::from(actual) - expected).abs() <= tolerance,
            "at ({x}, {y}): GPU {actual} != CPU {expected}"
        );
    }
}

#[tokio::test]
async fn production_shader_matches_cpu_at_pixel_centres_and_borders() {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&Default::default())
        .await
        .expect("GPU adapter");
    let (device, queue) = adapter
        .request_device(&Default::default())
        .await
        .expect("GPU device");
    let pipeline = pipeline(&device);
    let fixture: Fixture =
        serde_json::from_str(include_str!("../gljs.json")).expect("GL JS fixture");
    for case in fixture.cases {
        let image = RgbaImage::from_raw(case.dim, case.dim, case.rgba).expect("tile pixels");
        let dem = DemTile::from_image(&image, case.unpack).expect("DEM");
        assert_gpu_matches_cpu(&device, &queue, &pipeline, &dem);
        let image =
            RgbaImage::from_raw(case.dim, case.dim, case.neighbour_rgba).expect("neighbour pixels");
        let neighbour = DemTile::from_image(&image, case.unpack).expect("DEM");
        for border in case.backfilled {
            let mut filled = dem.clone();
            filled
                .backfill_border(&neighbour, border.dx, border.dy)
                .expect("border");
            assert_gpu_matches_cpu(&device, &queue, &pipeline, &filled);
        }
    }
}

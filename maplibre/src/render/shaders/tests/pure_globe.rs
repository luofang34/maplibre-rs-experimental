//! At a transition of one, every projection path returns the globe's clip position, however
//! undefined the inactive flat projection is.

use crate::render::projection::ShaderProjectionData;
use bytemuck::{bytes_of, cast_slice};

/// Calls the shared projection functions with a flat projection of infinities and stores what
/// they return.
const PROBE: &str = r#"
struct Inputs {
    flat: mat4x4<f32>,
    globe: vec4<f32>,
};

@group(0) @binding(1) var<storage, read> inputs: Inputs;
@group(0) @binding(2) var<storage, read_write> outputs: array<vec4<f32>, 4>;

@compute @workgroup_size(1)
fn probe() {
    let transition = projection.transition_and_padding.x;
    let flat = inputs.flat * vec4<f32>(1.0, 2.0, 3.0, 1.0);
    outputs[0] = blend_clip(flat, inputs.globe, transition);
    outputs[1] = interpolate_clip_position(flat, inputs.globe, transition);
    outputs[2] = project_tile_tangent_3d(vec3<f32>(0.5, 0.5, 100.0), vec2<f32>(0.01, 0.0),
        inputs.flat, vec4<f32>(0.0, 0.0, 1.0, 1.0));
    outputs[3] = project_tile_position_3d(vec3<f32>(0.25, 0.75, 100.0), inputs.flat,
        vec4<f32>(0.0, 0.0, 1.0, 1.0)).clip_position;
}
"#;

#[tokio::test]
async fn the_pure_globe_ignores_a_flat_projection_of_infinities() {
    let globe = [0.25_f32, -0.5, 0.75, 2.0];
    let mut inputs = [f32::INFINITY; 16].to_vec();
    inputs.extend(globe);
    let values = run_probe(
        ShaderProjectionData {
            transition: 1.0,
            ..ShaderProjectionData::default()
        },
        &inputs,
    )
    .await;
    assert!(
        values.iter().all(|value| value.is_finite()),
        "the flat projection's infinities reach the globe: {values:?}"
    );
    assert_eq!(
        &values[0..4],
        &globe,
        "blend_clip returns the globe position"
    );
    assert_eq!(
        &values[4..8],
        &globe,
        "interpolate_clip_position returns the globe position"
    );
}

/// [`PROBE`] after the projection functions it calls.
fn probe_pipeline(device: &wgpu::Device) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pure globe probe"),
        source: wgpu::ShaderSource::Wgsl(
            format!("{}\n{PROBE}", include_str!("../projection.vertex.wgsl")).into(),
        ),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("pure globe probe"),
        layout: None,
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// Runs [`PROBE`] once with `projection` and `inputs`, the flat matrix then the globe position,
/// and returns its four outputs.
async fn run_probe(projection: ShaderProjectionData, inputs: &[f32]) -> Vec<f32> {
    let adapter = wgpu::Instance::default()
        .request_adapter(&Default::default())
        .await
        .expect("GPU adapter");
    let (device, queue) = adapter
        .request_device(&Default::default())
        .await
        .expect("GPU device");
    let queue = crate::render::upload_queue::UploadQueue::new(queue);
    let pipeline = probe_pipeline(&device);
    let buffer = |contents: &[u8], usage| {
        queue.create_buffer_init(
            &device,
            &wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            },
        )
    };
    let uniform = buffer(bytes_of(&projection), wgpu::BufferUsages::UNIFORM);
    let input = buffer(cast_slice(inputs), wgpu::BufferUsages::STORAGE);
    let output = buffer(
        &[0; 64],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 64);
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU work completes");
    let values: Vec<f32> =
        cast_slice(&readback.slice(..).get_mapped_range().expect("range")).to_vec();
    values
}

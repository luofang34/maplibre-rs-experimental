use std::num::NonZeroU32;

use super::{
    create_slab, make_base_schedule, make_context, submit_pass, BindSharedSlab, DrawSlabIndexed,
    GpuBatchKey, GpuBatcher, GpuInputs, GpuInstance, PrimaryBinnedItem, SecondaryBinnedItem,
    SlabResources,
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{
            BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhasePlugin,
            BinnedRenderPhaseType, DrawFunctions, FallbackInstanceBufferMode,
            IndirectParametersBuffers, InputDataIndex, RenderCommand, RenderCommandResult,
        },
        tracked_pass::{DrawCapabilities, RenderStats, TrackedRenderPass},
        RenderStageLabel,
    },
    tcs::{system::SystemResult, world::World},
};

struct VStepSlabResources(SlabResources);

struct BindVStepSlab;

impl RenderCommand<SecondaryBinnedItem> for BindVStepSlab {
    fn render<'w>(
        world: &'w World,
        item: &SecondaryBinnedItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(VStepSlabResources(slab)) = world.resources.get::<VStepSlabResources>() else {
            return RenderCommandResult::Failure;
        };
        let Some(ib) = world.resources.get::<BatchedInstanceBuffer<GpuInstance>>() else {
            return RenderCommandResult::Failure;
        };
        let Some(slice) = ib.instance_slice(item.0.range.clone()) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(&slab.pipeline);
        pass.set_vertex_buffer(0, slab.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, slice);
        pass.set_index_buffer(slab.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        RenderCommandResult::Success
    }
}

#[tokio::test]
async fn simulated_webgl2_downlevel_fallbacks_and_compute_reserve_pass_validation() {
    let mut ctx = make_context().await;
    let native_caps =
        DrawCapabilities::from_adapter_and_device(&ctx.renderer.adapter, &ctx.renderer.device);

    // Verify compute storage indirect buffer reservation when device supports storage buffers
    if native_caps.indirect_execution && native_caps.vertex_storage_buffers {
        let scope_res = ctx
            .renderer
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let mut indirect = IndirectParametersBuffers::new();
        let idx_buf = indirect.reserve_indexed_gpu(&ctx.renderer.device, 16, native_caps);
        assert!(idx_buf.usage().contains(wgpu::BufferUsages::STORAGE));
        let non_idx_buf = indirect.reserve_non_indexed_gpu(&ctx.renderer.device, 16, native_caps);
        assert!(non_idx_buf.usage().contains(wgpu::BufferUsages::STORAGE));
        assert!(scope_res.pop().await.is_none());
    }

    let webgl_caps = DrawCapabilities::default();
    ctx.world.resources.insert(webgl_caps);
    ctx.world.resources.init::<RenderStats>();

    let dyn_shader = "
        @group(0) @binding(0) var<uniform> inst: vec4<f32>;
        @vertex fn vs(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos * 0.2 + inst.xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.2, 0.5, 0.9, 1.0); }
    ";
    let vstep_shader = "
        @vertex fn vs(@location(0) pos: vec2<f32>, @location(1) inst: vec4<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos * 0.2 + inst.xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.9, 0.5, 0.2, 1.0); }
    ";
    let ubo_size = BatchedInstanceBuffer::<GpuInstance>::uniform_binding_size().unwrap();
    let dyn_bgl = ctx
        .renderer
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("webgl_dyn_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: Some(ubo_size),
                },
                count: None,
            }],
        });
    ctx.world.resources.insert(create_slab(
        &ctx.renderer,
        dyn_shader,
        Some(&dyn_bgl),
        false,
        false,
    ));
    ctx.world.resources.insert(VStepSlabResources(create_slab(
        &ctx.renderer,
        vstep_shader,
        None,
        true,
        false,
    )));
    ctx.world.resources.insert(GpuInputs(vec![
        (GpuInstance([0.1, 0.0, 0.0, 1.0]), None),
        (GpuInstance([0.2, 0.0, 0.0, 1.0]), None),
        (GpuInstance([0.3, 0.0, 0.0, 1.0]), None),
    ]));

    // Install BOTH DynamicUniform (PrimaryBinnedItem) and VertexInstance (SecondaryBinnedItem)
    // sharing BatchedInstanceBuffer<GpuInstance> in the same schedule!
    let mut schedule = make_base_schedule();
    BinnedRenderPhasePlugin::<PrimaryBinnedItem, GpuBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::DynamicUniform)
        .install(&mut schedule, &mut ctx.world)
        .unwrap();
    BinnedRenderPhasePlugin::<SecondaryBinnedItem, GpuBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::VertexInstance)
        .install(&mut schedule, &mut ctx.world)
        .unwrap();

    let draw_dyn = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<PrimaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabIndexed)>();
    let draw_vstep = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<SecondaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindVStepSlab, DrawSlabIndexed)>();

    schedule
        .add_system_to_stage(
            RenderStageLabel::Queue,
            move |c: &mut MapContext| -> SystemResult {
                let p1 = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<PrimaryBinnedItem>>()
                    .unwrap();
                let key1 = GpuBatchKey {
                    pipeline_id: 0,
                    indexed: true,
                    draw_fn: draw_dyn,
                };
                p1.add_instanced(
                    key1.clone(),
                    0,
                    BinnedItemId(1),
                    InputDataIndex(0),
                    NonZeroU32::new(2).unwrap(),
                    BinnedRenderPhaseType::MultidrawableMesh,
                );
                p1.add(
                    key1,
                    1,
                    BinnedItemId(2),
                    InputDataIndex(2),
                    BinnedRenderPhaseType::BatchableMesh,
                );

                let p2 = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<SecondaryBinnedItem>>()
                    .unwrap();
                let key2 = GpuBatchKey {
                    pipeline_id: 1,
                    indexed: true,
                    draw_fn: draw_vstep,
                };
                p2.add_instanced(
                    key2.clone(),
                    0,
                    BinnedItemId(3),
                    InputDataIndex(0),
                    NonZeroU32::new(2).unwrap(),
                    BinnedRenderPhaseType::BatchableMesh,
                );
                // Bin 1 has non-zero start index in BatchedInstanceBuffer -> tests draw_binned_indexed 0..count!
                p2.add(
                    key2,
                    1,
                    BinnedItemId(4),
                    InputDataIndex(2),
                    BinnedRenderPhaseType::BatchableMesh,
                );
                Ok(())
            },
        )
        .unwrap();

    schedule
        .add_system_to_stage(
            RenderStageLabel::Render,
            move |c: &mut MapContext| -> SystemResult {
                let ib = c
                    .world
                    .resources
                    .get::<BatchedInstanceBuffer<GpuInstance>>()
                    .unwrap();
                let ubo_buf = ib.uniform_buffer().unwrap().clone();
                assert!(ib
                    .buffer()
                    .unwrap()
                    .usage()
                    .contains(wgpu::BufferUsages::VERTEX));
                assert!(ubo_buf.usage().contains(wgpu::BufferUsages::UNIFORM));

                let bg = c
                    .renderer
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &dyn_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &ubo_buf,
                                offset: 0,
                                size: Some(ubo_size),
                            }),
                        }],
                    });
                c.world
                    .resources
                    .get_mut::<SlabResources>()
                    .unwrap()
                    .bind_group = Some(bg);
                submit_pass(c, |pass, world| {
                    world
                        .resources
                        .get::<BinnedRenderPhase<PrimaryBinnedItem>>()
                        .unwrap()
                        .render(pass, world);
                    world
                        .resources
                        .get::<BinnedRenderPhase<SecondaryBinnedItem>>()
                        .unwrap()
                        .render(pass, world);
                });
                Ok(())
            },
        )
        .unwrap();

    let _ = ctx.renderer.queue.take_written_bytes();
    let scope = ctx
        .renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    schedule.run_once(&mut ctx).unwrap();
    assert!(scope.pop().await.is_none());

    let stride = ctx
        .world
        .resources
        .get::<BatchedInstanceBuffer<GpuInstance>>()
        .unwrap()
        .aligned_uniform_stride() as u64;
    // 3 dynamic uniform slots (3 * stride) + 6 packed instances in gpu_buffer (6 * 16)
    assert_eq!(ctx.renderer.queue.take_written_bytes(), 3 * stride + 6 * 16);
    assert_eq!(
        ctx.world
            .resources
            .get::<RenderStats>()
            .unwrap()
            .take()
            .draws,
        5
    );
}

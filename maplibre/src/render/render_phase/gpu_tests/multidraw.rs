use super::{
    bin_geometry, create_slab, make_base_schedule, make_context, submit_pass, BindSharedSlab,
    DrawSlabIndexed, DrawSlabNonIndexed, GpuBatchKey, GpuBatcher, GpuInputs, GpuInstance,
    PrimaryBinnedItem, SecondaryBinnedItem, SlabResources,
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{
            BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhasePlugin,
            BinnedRenderPhaseType, DrawFunctions, IndirectMeshMetadata, IndirectParametersBuffers,
            InputDataIndex,
        },
        tracked_pass::{DrawCapabilities, PassStats, RenderStats},
        RenderStageLabel,
    },
    tcs::system::{SystemError, SystemResult},
};

#[tokio::test]
async fn closed_loop_storage_multidraw_single_and_failure_cleanup_pass_validation() {
    let mut ctx = make_context().await;
    let native_caps =
        DrawCapabilities::from_adapter_and_device(&ctx.renderer.adapter, &ctx.renderer.device);
    if !native_caps.vertex_storage_buffers {
        return;
    }

    let storage_shader = "
        @group(0) @binding(0) var<storage, read> instances: array<vec4<f32>>;
        @vertex fn vs(@location(0) pos: vec2<f32>, @builtin(instance_index) inst: u32) -> @builtin(position) vec4<f32> {
            let d = instances[inst];
            return vec4<f32>(pos * 0.2 + d.xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.1, 0.9, 0.3, 1.0); }
    ";
    let bgl = ctx
        .renderer
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("storage_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
    ctx.world.resources.insert(create_slab(
        &ctx.renderer,
        storage_shader,
        Some(&bgl),
        false,
        native_caps.base_vertex,
    ));
    ctx.world.resources.init::<RenderStats>();

    let mut schedule = make_base_schedule();
    BinnedRenderPhasePlugin::<PrimaryBinnedItem, GpuBatcher>::new()
        .install(&mut schedule, &mut ctx.world)
        .unwrap();
    BinnedRenderPhasePlugin::<SecondaryBinnedItem, GpuBatcher>::new()
        .install(&mut schedule, &mut ctx.world)
        .unwrap();

    let primary_draw = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<PrimaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabIndexed)>();
    let secondary_draw = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<SecondaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabNonIndexed)>();

    let mut inputs = GpuInputs::default();
    for bin in 0..3u32 {
        let (index_range, base_vertex) = bin_geometry(bin, native_caps.base_vertex);
        inputs.0.push((
            GpuInstance([bin as f32 * 0.1, 0.0, 0.0, 1.0]),
            Some(IndirectMeshMetadata::Indexed {
                index_range,
                base_vertex,
            }),
        ));
    }
    inputs.0.push((
        GpuInstance([0.0, 0.2, 0.0, 1.0]),
        Some(IndirectMeshMetadata::NonIndexed { vertex_range: 0..3 }),
    ));
    ctx.world.resources.insert(inputs);

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
                    draw_fn: primary_draw,
                };
                for bin in [2u32, 0, 1] {
                    p1.add(
                        key1.clone(),
                        bin,
                        BinnedItemId(bin as u64),
                        InputDataIndex(bin),
                        BinnedRenderPhaseType::MultidrawableMesh,
                    );
                }
                let p2 = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<SecondaryBinnedItem>>()
                    .unwrap();
                p2.add(
                    GpuBatchKey {
                        pipeline_id: 0,
                        indexed: false,
                        draw_fn: secondary_draw,
                    },
                    0,
                    BinnedItemId(99),
                    InputDataIndex(3),
                    BinnedRenderPhaseType::BatchableMesh,
                );
                Ok(())
            },
        )
        .unwrap();

    let bgl_clone = bgl.clone();
    schedule
        .add_system_to_stage(
            RenderStageLabel::Render,
            move |c: &mut MapContext| -> SystemResult {
                let inst_buf = c
                    .world
                    .resources
                    .get::<BatchedInstanceBuffer<GpuInstance>>()
                    .unwrap()
                    .buffer()
                    .unwrap()
                    .clone();
                let bg = c
                    .renderer
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &bgl_clone,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: inst_buf.as_entire_binding(),
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

    ctx.world.resources.insert(DrawCapabilities {
        multi_draw_indirect: false,
        indirect_execution: false,
        ..native_caps
    });
    let _ = ctx.renderer.queue.take_written_bytes();
    let _ = ctx.world.resources.get::<RenderStats>().unwrap().take();

    let scope = ctx
        .renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    schedule.run_once(&mut ctx).unwrap();
    assert!(scope.pop().await.is_none());

    assert_eq!(
        ctx.world.resources.get::<RenderStats>().unwrap().take(),
        PassStats {
            draws: 4,
            state_changes: 4,
            redundant_state_changes: 11
        }
    );
    assert_eq!(ctx.renderer.queue.take_written_bytes(), 64);
    assert!(ctx
        .world
        .resources
        .get::<BinnedRenderPhase<PrimaryBinnedItem>>()
        .unwrap()
        .is_empty());

    if native_caps.supports_multidraw_indirect(true) {
        ctx.world.resources.insert(native_caps);
        if native_caps.multi_draw_indirect_count {
            ctx.world
                .resources
                .get_mut::<IndirectParametersBuffers>()
                .unwrap()
                .set_gpu_count_enabled(true);
        }
        let scope_md = ctx
            .renderer
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        schedule.run_once(&mut ctx).unwrap();
        assert!(scope_md.pop().await.is_none());
        assert_eq!(
            ctx.world.resources.get::<RenderStats>().unwrap().take(),
            PassStats {
                draws: 2,
                state_changes: 4,
                redundant_state_changes: 3
            }
        );
        assert_eq!(ctx.renderer.queue.take_written_bytes(), 64 + 60 + 8 + 16);
    }

    schedule
        .add_system_to_stage(
            RenderStageLabel::PhaseSort,
            |_c: &mut MapContext| -> SystemResult { Err(SystemError::Dependencies) },
        )
        .unwrap();
    assert!(schedule.run_once(&mut ctx).is_err());
    assert!(ctx
        .world
        .resources
        .get::<BinnedRenderPhase<PrimaryBinnedItem>>()
        .unwrap()
        .is_empty());
    assert!(ctx
        .world
        .resources
        .get::<BinnedRenderPhase<SecondaryBinnedItem>>()
        .unwrap()
        .is_empty());
}

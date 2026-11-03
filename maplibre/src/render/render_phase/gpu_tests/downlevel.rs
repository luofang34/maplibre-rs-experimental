use std::num::NonZeroU32;

use super::{
    create_slab, make_base_schedule, make_context, submit_pass, BindSharedSlab, DrawSlabIndexed,
    GpuBatchKey, GpuBatcher, GpuInputs, GpuInstance, PrimaryBinnedItem, SlabResources,
};
use crate::{
    context::MapContext,
    render::{
        render_phase::{
            BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhasePlugin,
            BinnedRenderPhaseType, DrawFunctions, FallbackInstanceBufferMode,
            IndirectParametersBuffers, InputDataIndex,
        },
        tracked_pass::{DrawCapabilities, RenderStats},
        RenderStageLabel,
    },
    tcs::system::SystemResult,
};

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

    // Mode A: DynamicUniform fallback
    let dyn_shader = "
        @group(0) @binding(0) var<uniform> inst: vec4<f32>;
        @vertex fn vs(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos * 0.2 + inst.xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.2, 0.5, 0.9, 1.0); }
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
    ctx.world.resources.insert(GpuInputs(vec![
        (GpuInstance([0.1, 0.0, 0.0, 1.0]), None),
        (GpuInstance([0.2, 0.0, 0.0, 1.0]), None),
        (GpuInstance([0.3, 0.0, 0.0, 1.0]), None),
    ]));

    let mut schedule = make_base_schedule();
    BinnedRenderPhasePlugin::<PrimaryBinnedItem, GpuBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::DynamicUniform)
        .install(&mut schedule, &mut ctx.world)
        .unwrap();
    let draw_id = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<PrimaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabIndexed)>();

    schedule
        .add_system_to_stage(
            RenderStageLabel::Queue,
            move |c: &mut MapContext| -> SystemResult {
                let p = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<PrimaryBinnedItem>>()
                    .unwrap();
                let key = GpuBatchKey {
                    pipeline_id: 0,
                    indexed: true,
                    draw_fn: draw_id,
                };
                p.add_instanced(
                    key.clone(),
                    0,
                    BinnedItemId(1),
                    InputDataIndex(0),
                    NonZeroU32::new(2).unwrap(),
                    BinnedRenderPhaseType::MultidrawableMesh,
                );
                p.add(
                    key,
                    1,
                    BinnedItemId(2),
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
                let ubo_buf = c
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
                });
                Ok(())
            },
        )
        .unwrap();

    let _ = ctx.renderer.queue.take_written_bytes();
    let scope_dyn = ctx
        .renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    schedule.run_once(&mut ctx).unwrap();
    assert!(scope_dyn.pop().await.is_none());

    let stride = ctx
        .world
        .resources
        .get::<BatchedInstanceBuffer<GpuInstance>>()
        .unwrap()
        .aligned_uniform_stride() as u64;
    assert_eq!(ctx.renderer.queue.take_written_bytes(), 3 * stride);
    assert_eq!(
        ctx.world
            .resources
            .get::<RenderStats>()
            .unwrap()
            .take()
            .draws,
        3
    );

    // Mode B: VertexStepMode::Instance fallback with zero dynamic uniforms & first_instance == 0
    let vstep_shader = "
        @vertex fn vs(@location(0) pos: vec2<f32>, @location(1) inst: vec4<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos * 0.2 + inst.xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.9, 0.5, 0.2, 1.0); }
    ";
    ctx.world
        .resources
        .insert(create_slab(&ctx.renderer, vstep_shader, None, true, false));
    let mut vstep_schedule = make_base_schedule();
    BinnedRenderPhasePlugin::<PrimaryBinnedItem, GpuBatcher>::new()
        .with_fallback_mode(FallbackInstanceBufferMode::VertexInstance)
        .install(&mut vstep_schedule, &mut ctx.world)
        .unwrap();
    vstep_schedule
        .add_system_to_stage(
            RenderStageLabel::Queue,
            move |c: &mut MapContext| -> SystemResult {
                let p = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<PrimaryBinnedItem>>()
                    .unwrap();
                let key = GpuBatchKey {
                    pipeline_id: 1,
                    indexed: true,
                    draw_fn: draw_id,
                };
                p.add_instanced(
                    key.clone(),
                    0,
                    BinnedItemId(1),
                    InputDataIndex(0),
                    NonZeroU32::new(2).unwrap(),
                    BinnedRenderPhaseType::BatchableMesh,
                );
                p.add(
                    key,
                    1,
                    BinnedItemId(2),
                    InputDataIndex(2),
                    BinnedRenderPhaseType::BatchableMesh,
                );
                Ok(())
            },
        )
        .unwrap();
    vstep_schedule
        .add_system_to_stage(
            RenderStageLabel::Render,
            |c: &mut MapContext| -> SystemResult {
                submit_pass(c, |pass, world| {
                    world
                        .resources
                        .get::<BinnedRenderPhase<PrimaryBinnedItem>>()
                        .unwrap()
                        .render(pass, world);
                });
                Ok(())
            },
        )
        .unwrap();

    let _ = ctx.renderer.queue.take_written_bytes();
    let scope_vstep = ctx
        .renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    vstep_schedule.run_once(&mut ctx).unwrap();
    assert!(scope_vstep.pop().await.is_none());
    assert_eq!(ctx.renderer.queue.take_written_bytes(), 3 * 16);
    assert_eq!(
        ctx.world
            .resources
            .get::<RenderStats>()
            .unwrap()
            .take()
            .draws,
        2
    );
}

#[cfg(feature = "headless")]
#[tokio::test]
async fn plugin_build_single_indexed_and_multidraw_non_indexed_indirect_pass_validation() {
    use std::rc::Rc;

    use super::{
        bin_geometry, create_slab, make_base_schedule, make_context, submit_pass, BindSharedSlab,
        DrawSlabIndexed, DrawSlabNonIndexed, GpuBatchKey, GpuBatcher, GpuInputs, GpuInstance,
        PrimaryBinnedItem, SecondaryBinnedItem, SlabResources,
    };
    use crate::{
        context::MapContext,
        headless::{create_headless_renderer, environment::HeadlessEnvironment},
        plugin::Plugin,
        render::{
            render_phase::{
                BatchedInstanceBuffer, BinnedItemId, BinnedRenderPhase, BinnedRenderPhasePlugin,
                BinnedRenderPhaseType, DrawFunctions, IndirectMeshMetadata, InputDataIndex,
            },
            tracked_pass::{DrawCapabilities, RenderStats},
            RenderStageLabel,
        },
        tcs::{system::SystemResult, world::World},
    };

    let (kernel, mut renderer) = create_headless_renderer(16, 16, None).await.unwrap();
    let kernel = Rc::new(kernel);
    let mut schedule = make_base_schedule();
    let mut world = World::default();

    let plugin = BinnedRenderPhasePlugin::<SecondaryBinnedItem, GpuBatcher>::new();
    Plugin::<HeadlessEnvironment>::build(&plugin, &mut schedule, kernel, &mut world, &mut renderer)
        .unwrap();
    assert!(world
        .resources
        .exists::<BinnedRenderPhase<SecondaryBinnedItem>>());

    let mut ctx = make_context().await;
    let native_caps =
        DrawCapabilities::from_adapter_and_device(&ctx.renderer.adapter, &ctx.renderer.device);
    if !native_caps.supports_multidraw_indirect(false) || !native_caps.vertex_storage_buffers {
        return;
    }

    let shader = "
        @group(0) @binding(0) var<storage, read> instances: array<vec4<f32>>;
        @vertex fn vs(@location(0) pos: vec2<f32>, @builtin(instance_index) inst: u32) -> @builtin(position) vec4<f32> {
            return vec4<f32>(pos * 0.2 + instances[inst].xy, 0.0, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.5, 0.5, 0.9, 1.0); }
    ";
    let bgl = ctx
        .renderer
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        shader,
        Some(&bgl),
        false,
        native_caps.base_vertex,
    ));
    ctx.world.resources.init::<RenderStats>();

    let (idx_range, base_vertex) = bin_geometry(0, native_caps.base_vertex);
    ctx.world.resources.insert(GpuInputs(vec![
        (
            GpuInstance([0.0, 0.0, 0.0, 1.0]),
            Some(IndirectMeshMetadata::Indexed {
                index_range: idx_range,
                base_vertex,
            }),
        ),
        (
            GpuInstance([0.1, 0.0, 0.0, 1.0]),
            Some(IndirectMeshMetadata::NonIndexed { vertex_range: 0..3 }),
        ),
        (
            GpuInstance([0.2, 0.0, 0.0, 1.0]),
            Some(IndirectMeshMetadata::NonIndexed { vertex_range: 3..6 }),
        ),
    ]));

    let mut sched = make_base_schedule();
    BinnedRenderPhasePlugin::<PrimaryBinnedItem, GpuBatcher>::new()
        .install(&mut sched, &mut ctx.world)
        .unwrap();
    BinnedRenderPhasePlugin::<SecondaryBinnedItem, GpuBatcher>::new()
        .install(&mut sched, &mut ctx.world)
        .unwrap();
    let d1 = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<PrimaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabIndexed)>();
    let d2 = ctx
        .world
        .resources
        .get_mut::<DrawFunctions<SecondaryBinnedItem>>()
        .unwrap()
        .add_with::<(BindSharedSlab, DrawSlabNonIndexed)>();

    sched
        .add_system_to_stage(
            RenderStageLabel::Queue,
            move |c: &mut MapContext| -> SystemResult {
                // Single BatchableMesh indexed bin -> exercises draw_indexed_indirect!
                c.world
                    .resources
                    .get_mut::<BinnedRenderPhase<PrimaryBinnedItem>>()
                    .unwrap()
                    .add(
                        GpuBatchKey {
                            pipeline_id: 0,
                            indexed: true,
                            draw_fn: d1,
                        },
                        0,
                        BinnedItemId(1),
                        InputDataIndex(0),
                        BinnedRenderPhaseType::BatchableMesh,
                    );
                // Two MultidrawableMesh non-indexed bins -> exercises multi_draw_indirect!
                let p2 = c
                    .world
                    .resources
                    .get_mut::<BinnedRenderPhase<SecondaryBinnedItem>>()
                    .unwrap();
                let k2 = GpuBatchKey {
                    pipeline_id: 0,
                    indexed: false,
                    draw_fn: d2,
                };
                p2.add(
                    k2.clone(),
                    0,
                    BinnedItemId(2),
                    InputDataIndex(1),
                    BinnedRenderPhaseType::MultidrawableMesh,
                );
                p2.add(
                    k2,
                    1,
                    BinnedItemId(3),
                    InputDataIndex(2),
                    BinnedRenderPhaseType::MultidrawableMesh,
                );
                Ok(())
            },
        )
        .unwrap();

    sched
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
                        layout: &bgl,
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

    ctx.world.resources.insert(native_caps);
    let scope = ctx
        .renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    sched.run_once(&mut ctx).unwrap();
    assert!(scope.pop().await.is_none());
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

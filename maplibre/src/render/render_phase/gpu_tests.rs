#![allow(clippy::expect_used, clippy::unwrap_used)]

mod downlevel;
mod multidraw;
mod plugin;

use std::ops::Range;

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    context::MapContext,
    coords::{WorldCoords, Zoom},
    render::{
        render_phase::{
            draw_binned_indexed, draw_binned_non_indexed, BatchedInstanceBuffer, BinnedItemId,
            BinnedPhaseItem, DrawFunctionId, GetBatchData, IndirectMeshMetadata, InputDataIndex,
            PhaseItemBatchSetKey, PhaseItemExtraIndex, RenderCommand, RenderCommandResult,
        },
        settings::{RendererSettings, WgpuSettings},
        tracked_pass::TrackedRenderPass,
        view_state::ViewState,
        RenderStageLabel, Renderer,
    },
    schedule::Schedule,
    tcs::{system::stage::SystemStage, world::World},
    window::{MapWindow, PhysicalSize},
};

pub(super) struct OffscreenWindow(pub(super) PhysicalSize);

impl MapWindow for OffscreenWindow {
    fn size(&self) -> PhysicalSize {
        self.0
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct GpuInstance(pub(super) [f32; 4]);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct GpuBatchKey {
    pub(super) pipeline_id: u32,
    pub(super) indexed: bool,
    pub(super) draw_fn: DrawFunctionId,
}

impl PhaseItemBatchSetKey for GpuBatchKey {
    fn indexed(&self) -> bool {
        self.indexed
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.draw_fn
    }
}

#[derive(Clone, Debug)]
pub(super) struct PrimaryBinnedItem {
    pub(super) key: GpuBatchKey,
    pub(super) bin: u32,
    pub(super) range: Range<u32>,
    pub(super) extra: PhaseItemExtraIndex,
}

impl BinnedPhaseItem for PrimaryBinnedItem {
    type BatchSetKey = GpuBatchKey;
    type BinKey = u32;

    fn new(
        key: Self::BatchSetKey,
        bin: Self::BinKey,
        _representative_item: BinnedItemId,
        range: Range<u32>,
        extra: PhaseItemExtraIndex,
    ) -> Self {
        Self {
            key,
            bin,
            range,
            extra,
        }
    }

    fn batch_range(&self) -> &Range<u32> {
        &self.range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra.clone()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.key.draw_fn
    }
}

#[derive(Clone, Debug)]
pub(super) struct SecondaryBinnedItem(pub(super) PrimaryBinnedItem);

impl BinnedPhaseItem for SecondaryBinnedItem {
    type BatchSetKey = GpuBatchKey;
    type BinKey = u32;

    fn new(
        key: Self::BatchSetKey,
        bin: Self::BinKey,
        rep: BinnedItemId,
        range: Range<u32>,
        extra: PhaseItemExtraIndex,
    ) -> Self {
        Self(PrimaryBinnedItem::new(key, bin, rep, range, extra))
    }

    fn batch_range(&self) -> &Range<u32> {
        self.0.batch_range()
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.0.extra_index()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.0.draw_function()
    }
}

#[derive(Default)]
pub(super) struct GpuInputs(pub(super) Vec<(GpuInstance, Option<IndirectMeshMetadata>)>);

pub(super) struct GpuBatcher;

impl GetBatchData<PrimaryBinnedItem> for GpuBatcher {
    type BufferData = GpuInstance;

    fn get_batch_data(
        world: &World,
        _item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        world
            .resources
            .get::<GpuInputs>()
            .and_then(|s| s.0.get(input_index.0 as usize).cloned())
    }
}

impl GetBatchData<SecondaryBinnedItem> for GpuBatcher {
    type BufferData = GpuInstance;

    fn get_batch_data(
        world: &World,
        item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        <Self as GetBatchData<PrimaryBinnedItem>>::get_batch_data(world, item, input_index)
    }
}

pub(super) struct SlabResources {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) bind_group: Option<wgpu::BindGroup>,
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) target_view: wgpu::TextureView,
    pub(super) use_base_vertex: bool,
    pub(super) vertex_instance_step: bool,
}

pub(super) fn bin_geometry(bin: u32, use_base_vertex: bool) -> (Range<u32>, i32) {
    let first_idx = bin * 3;
    let base_vertex = if use_base_vertex { (bin * 3) as i32 } else { 0 };
    (first_idx..(first_idx + 3), base_vertex)
}

pub(super) struct BindSharedSlab;
pub(super) struct DrawSlabIndexed;
pub(super) struct DrawSlabNonIndexed;

impl RenderCommand<PrimaryBinnedItem> for BindSharedSlab {
    fn render<'w>(
        world: &'w World,
        item: &PrimaryBinnedItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(slab) = world.resources.get::<SlabResources>() else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(&slab.pipeline);
        if let Some(bg) = &slab.bind_group {
            match item.extra_index().dynamic_offset() {
                Some(off) => pass.set_bind_group(0, bg, &[off]),
                None => pass.set_bind_group(0, bg, &[]),
            }
        }
        pass.set_vertex_buffer(0, slab.vertex_buffer.slice(..));
        if slab.vertex_instance_step {
            let Some(ib) = world.resources.get::<BatchedInstanceBuffer<GpuInstance>>() else {
                return RenderCommandResult::Failure;
            };
            let Some(slice) = ib.instance_slice(item.batch_range().clone()) else {
                return RenderCommandResult::Failure;
            };
            pass.set_vertex_buffer(1, slice);
        }
        if item.key.indexed {
            pass.set_index_buffer(slab.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        }
        RenderCommandResult::Success
    }
}

impl RenderCommand<PrimaryBinnedItem> for DrawSlabIndexed {
    fn render<'w>(
        world: &'w World,
        item: &PrimaryBinnedItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(slab) = world.resources.get::<SlabResources>() else {
            return RenderCommandResult::Failure;
        };
        let (indices, base_vertex) = bin_geometry(item.bin, slab.use_base_vertex);
        if slab.vertex_instance_step {
            let count = item
                .batch_range()
                .end
                .saturating_sub(item.batch_range().start);
            pass.draw_indexed(indices, 0, 0..count);
            RenderCommandResult::Success
        } else {
            draw_binned_indexed(pass, world, item, indices, base_vertex)
        }
    }
}

impl RenderCommand<SecondaryBinnedItem> for BindSharedSlab {
    fn render<'w>(
        world: &'w World,
        item: &SecondaryBinnedItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        <Self as RenderCommand<PrimaryBinnedItem>>::render(world, &item.0, pass)
    }
}

impl RenderCommand<SecondaryBinnedItem> for DrawSlabNonIndexed {
    fn render<'w>(
        world: &'w World,
        item: &SecondaryBinnedItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        draw_binned_non_indexed(pass, world, &item.0, 0..3)
    }
}

pub(super) async fn make_context() -> MapContext {
    let size = PhysicalSize::new(16, 16).expect("nonzero");
    let renderer = Renderer::initialize_headless(
        &OffscreenWindow(size),
        WgpuSettings::default(),
        RendererSettings::default(),
    )
    .await
    .expect("headless renderer");
    MapContext {
        renderer,
        style: Default::default(),
        world: World::default(),
        view_state: ViewState::new(
            size,
            WorldCoords::from((0.0, 0.0)),
            Zoom::new(0.0),
            cgmath::Deg(0.0),
            cgmath::Rad(0.64),
        ),
    }
}

pub(super) fn make_base_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    for stage in [
        RenderStageLabel::Prepare,
        RenderStageLabel::Queue,
        RenderStageLabel::PhaseSort,
        RenderStageLabel::Render,
        RenderStageLabel::Cleanup,
    ] {
        schedule.add_stage(stage, SystemStage::default()).unwrap();
    }
    schedule
        .run_after_failure(RenderStageLabel::Cleanup)
        .unwrap();
    schedule
}

pub(super) fn create_slab(
    renderer: &Renderer,
    shader_wgsl: &str,
    bgl: Option<&wgpu::BindGroupLayout>,
    vertex_instance_step: bool,
    use_base_vertex: bool,
) -> SlabResources {
    let device = renderer.device();
    let vertices: [[f32; 2]; 9] = [
        [-0.5, -0.5],
        [0.5, -0.5],
        [0.0, 0.5],
        [-0.4, -0.4],
        [0.4, -0.4],
        [0.0, 0.4],
        [-0.3, -0.3],
        [0.3, -0.3],
        [0.0, 0.3],
    ];
    let indices: [u32; 9] = [0, 1, 2, 0, 1, 2, 0, 1, 2];
    let vertex_buffer = renderer.queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test_slab_vb"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );
    let index_buffer = renderer.queue.create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test_slab_ib"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        },
    );
    let target_view = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("test_binned_target"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test_binned_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_wgsl.into()),
    });
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = bgl.into_iter().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &groups,
        ..Default::default()
    });
    let mut vb_layouts = vec![Some(wgpu::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
    })];
    let inst_attrs = wgpu::vertex_attr_array![1 => Float32x4];
    if vertex_instance_step {
        vb_layouts.push(Some(wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &inst_attrs,
        }));
    }
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("test_binned_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &vb_layouts,
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    SlabResources {
        pipeline,
        bind_group: None,
        vertex_buffer,
        index_buffer,
        target_view,
        use_base_vertex,
        vertex_instance_step,
    }
}

pub(super) fn submit_pass(
    c: &mut MapContext,
    record: impl for<'w> FnOnce(&mut TrackedRenderPass<'w>, &'w World),
) {
    let view = c
        .world
        .resources
        .get::<SlabResources>()
        .unwrap()
        .target_view
        .clone();
    let mut encoder = c
        .renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let raw_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("binned_test_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut pass = TrackedRenderPass::new(raw_pass);
        record(&mut pass, &c.world);
        pass.finish(&c.world);
    }
    c.renderer.queue.submit([encoder.finish()]);
}

use super::frame;
use crate::{
    context::MapContext,
    headless::{
        create_headless_renderer,
        map::{xr::XrFrameError, HeadlessMap},
    },
    render::{
        eye_covering::{EyeInFrame, EyePassCount},
        render_phase::{
            BinnedItemId, BinnedPhaseItem, BinnedRenderPhase, BinnedRenderPhasePlugin,
            BinnedRenderPhaseType, DrawFunctionId, GetBatchData, IndirectMeshMetadata,
            InputDataIndex, PhaseItemBatchSetKey, PhaseItemExtraIndex,
        },
        RenderPlugin, RenderStageLabel,
    },
    style::Style,
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct XrBatchSetKey(u32);

impl PhaseItemBatchSetKey for XrBatchSetKey {
    fn indexed(&self) -> bool {
        false
    }

    fn draw_function(&self) -> DrawFunctionId {
        DrawFunctionId(self.0)
    }
}

struct XrBinnedItem {
    key: XrBatchSetKey,
    batch_range: std::ops::Range<u32>,
    extra_index: PhaseItemExtraIndex,
}

impl BinnedPhaseItem for XrBinnedItem {
    type BatchSetKey = XrBatchSetKey;
    type BinKey = u32;

    fn new(
        batch_set_key: Self::BatchSetKey,
        _bin_key: Self::BinKey,
        _representative_item: BinnedItemId,
        batch_range: std::ops::Range<u32>,
        extra_index: PhaseItemExtraIndex,
    ) -> Self {
        Self {
            key: batch_set_key,
            batch_range,
            extra_index,
        }
    }

    fn batch_range(&self) -> &std::ops::Range<u32> {
        &self.batch_range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra_index.clone()
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.key.draw_function()
    }
}

impl GetBatchData<XrBinnedItem> for XrBinnedItem {
    type BufferData = [f32; 4];

    fn get_batch_data(
        _world: &World,
        _item: BinnedItemId,
        input_index: InputDataIndex,
    ) -> Option<(Self::BufferData, Option<IndirectMeshMetadata>)> {
        Some(([input_index.0 as f32, 0.0, 0.0, 1.0], None))
    }
}

#[derive(Clone, Copy, Default)]
struct XrBinnedControl {
    queue_right_eye: bool,
    fail_eye_zero: bool,
}

fn queue_xr_binned(context: &mut MapContext) -> SystemResult {
    let ctl = context
        .world
        .resources
        .get::<XrBinnedControl>()
        .copied()
        .unwrap_or_default();
    let eye = context.world.resources.get::<EyeInFrame>().copied();
    let phase = context
        .world
        .resources
        .get_mut::<BinnedRenderPhase<XrBinnedItem>>()
        .expect("binned phase");
    if ctl.fail_eye_zero && eye.is_some_and(|e| e.index == 0) {
        phase.add(
            XrBatchSetKey(1),
            77,
            BinnedItemId(1),
            InputDataIndex(0),
            BinnedRenderPhaseType::BatchableMesh,
        );
        return Err(SystemError::Dependencies);
    }
    if ctl.queue_right_eye && eye.is_some_and(|e| e.index == 1) {
        for i in 0..8 {
            phase.add(
                XrBatchSetKey(1),
                99,
                BinnedItemId(10 + i),
                InputDataIndex(i as u32),
                BinnedRenderPhaseType::BatchableMesh,
            );
        }
    }
    Ok(())
}

#[test]
fn xr_frame_right_eye_bin_zero_inactive_retention_and_error_pass_count_cleanup() {
    crate::headless::map::test_async::run_async(async {
        let style: Style =
            serde_json::from_str(r#"{"version":8,"sources":{},"layers":[]}"#).expect("style");
        let (kernel, renderer) = create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(
                    BinnedRenderPhasePlugin::<XrBinnedItem, XrBinnedItem>::new()
                        .with_max_inactive_generations(0),
                ),
            ],
        )
        .expect("map");
        map.schedule
            .add_system_to_stage(RenderStageLabel::Queue, queue_xr_binned)
            .expect("schedule");

        map.map_context.world.resources.insert(XrBinnedControl {
            queue_right_eye: true,
            fail_eye_zero: false,
        });
        let mut expected_cap = None;
        for ts in [16, 32, 48] {
            map.run_xr_frame(frame(ts)).expect("stereo frame");
            assert!(!map.world().resources.exists::<EyePassCount>());
            let cap = map
                .world()
                .resources
                .get::<BinnedRenderPhase<XrBinnedItem>>()
                .expect("phase")
                .bin_capacity(&XrBatchSetKey(1), &99, BinnedRenderPhaseType::BatchableMesh)
                .expect("right-eye bin kept across stereo frames");
            assert!(cap >= 8);
            assert_eq!(*expected_cap.get_or_insert(cap), cap);
        }

        map.map_context.world.resources.insert(XrBinnedControl {
            queue_right_eye: false,
            fail_eye_zero: false,
        });
        map.run_xr_frame(frame(64)).expect("empty stereo frame");
        assert_eq!(
            map.world()
                .resources
                .get::<BinnedRenderPhase<XrBinnedItem>>()
                .expect("phase")
                .allocated_bin_count(),
            0
        );

        map.map_context.world.resources.insert(XrBinnedControl {
            queue_right_eye: false,
            fail_eye_zero: true,
        });
        assert!(matches!(
            map.run_xr_frame(frame(80)),
            Err(XrFrameError::Eye { index: 0, .. })
        ));
        assert!(!map.world().resources.exists::<EyePassCount>());
        let gen_after_err = map
            .world()
            .resources
            .get::<BinnedRenderPhase<XrBinnedItem>>()
            .expect("phase")
            .generation();

        map.map_context.world.resources.insert(XrBinnedControl {
            queue_right_eye: false,
            fail_eye_zero: false,
        });
        map.run_frame().expect("normal frame after XR error");
        map.run_frame().expect("second normal frame after XR error");
        let phase = map
            .world()
            .resources
            .get::<BinnedRenderPhase<XrBinnedItem>>()
            .expect("phase");
        assert_eq!(phase.generation(), gen_after_err + 2);
        assert_eq!(phase.allocated_bin_count(), 0);
    })
}

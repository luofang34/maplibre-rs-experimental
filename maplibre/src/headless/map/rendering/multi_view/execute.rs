//! Schedule execution across active and skipped [`ViewSlotId`]s for a [`MultiViewFrame`].

use rustc_hash::FxHashSet;

use super::{
    covering::{build_multi_view_coverings, build_slot_view_state, record_slot_post_render},
    swap_slot_resources, CoveringPolicy, MultiViewError, MultiViewFrame, MultiViewRegistry,
    SlotView, ViewSlotEntry,
};
use crate::{
    headless::map::HeadlessMap,
    render::{
        eye_covering::{
            snapshot_lod_history, CurrentViewSlot, DrapeView, EyeInFrame, MultiViewExtractCovering,
            SlotResidencyFloor,
        },
        frame_input::{FrameInput, ViewSource},
        view_state::ViewState,
    },
    schedule::{RenderStageLabel, StageLabel},
    sdf::provided::DisplayPixelRatio,
};

impl HeadlessMap {
    fn collect_active_slots(
        &self,
        frame: &MultiViewFrame,
    ) -> Result<Vec<(ViewSlotEntry, ViewState)>, MultiViewError> {
        let scale_factor = self
            .map_context
            .world
            .resources
            .get::<DisplayPixelRatio>()
            .map_or(1.0, |ratio| f64::from(ratio.0));

        let mut seen_independent = FxHashSet::default();
        let mut active = Vec::with_capacity(frame.slots.len());
        for entry in &frame.slots {
            let slot_size = self
                .slot_ref(entry.slot)
                .ok_or(MultiViewError::UnknownSlot { slot: entry.slot })?
                .surface
                .size();
            match entry.policy {
                CoveringPolicy::Independent => {
                    seen_independent.insert(entry.slot);
                }
                CoveringPolicy::StereoReuse(primary) => {
                    if !seen_independent.contains(&primary) {
                        return Err(MultiViewError::InvalidStereoPrimary {
                            slot: entry.slot,
                            primary,
                        });
                    }
                }
            }
            let view_state = build_slot_view_state(
                &self.map_context.view_state,
                &self.map_context.style,
                &self.map_context.world,
                slot_size,
                scale_factor,
                (frame.opaque_environment, frame.request_overscan),
                &entry.view,
            )?;
            active.push((*entry, view_state));
        }
        Ok(active)
    }

    /// Renders all active slots in `frame` over one shared `World`, retaining tiles and drapes
    /// for both active and skipped registered slots.
    pub fn run_multi_view_frame(&mut self, frame: MultiViewFrame) -> Result<(), MultiViewError> {
        self.map_context
            .world
            .resources
            .get_or_init_mut::<FrameInput>()
            .timestamp = frame.timestamp;
        if frame.slots.is_empty() {
            return Ok(());
        }

        let active = self.collect_active_slots(&frame)?;
        let mut registry = std::mem::take(
            self.map_context
                .world
                .resources
                .get_or_init_mut::<MultiViewRegistry>(),
        );
        registry.frame_counter = registry.frame_counter.wrapping_add(1);
        let frame_number = registry.frame_counter;

        let result = self.execute_multi_view(&frame, &active, &mut registry, frame_number);

        self.map_context
            .world
            .resources
            .insert(MultiViewExtractCovering::default());
        self.map_context
            .world
            .resources
            .insert(SlotResidencyFloor::default());
        self.map_context
            .world
            .resources
            .insert(CurrentViewSlot(None));
        self.map_context.world.resources.insert(DrapeView(None));
        self.map_context.world.resources.insert(EyeInFrame {
            index: 0,
            frame: frame_number,
        });
        self.map_context.world.resources.insert(registry);
        result
    }

    fn begin_multi_view_frame(
        &mut self,
        frame: &MultiViewFrame,
        active: &[(ViewSlotEntry, ViewState)],
        registry: &mut MultiViewRegistry,
        frame_number: u64,
    ) -> Result<(ViewState, SlotResidencyFloor), MultiViewError> {
        let saved_view_state = self.map_context.view_state.clone();
        let (extract_covering, floor) = build_multi_view_coverings(
            &self.map_context.style,
            &mut self.map_context.world,
            &mut registry.slots,
            active,
        );
        self.map_context.world.resources.insert(extract_covering);
        self.map_context.world.resources.insert(floor.clone());
        self.map_context.world.resources.insert(EyeInFrame {
            index: 0,
            frame: frame_number,
        });
        if let Some((first_entry, first_view)) = active.first() {
            self.map_context.view_state = first_view.clone();
            let input = self
                .map_context
                .world
                .resources
                .get_or_init_mut::<FrameInput>();
            input.timestamp = frame.timestamp;
            if let SlotView::External(ext) = first_entry.view {
                input.view = ViewSource::External(ext);
                input.request_overscan = frame.request_overscan;
            } else {
                input.view = ViewSource::MapView;
            }
        }
        crate::map::runtime::frame::run_stages_only(
            &mut self.schedule,
            &mut self.map_context,
            |label| {
                label == &RenderStageLabel::Input as &dyn StageLabel
                    || label == &RenderStageLabel::Request as &dyn StageLabel
            },
        )
        .map_err(|source| {
            self.map_context.view_state = saved_view_state.clone();
            MultiViewError::SlotStage { slot: None, source }
        })?;
        Ok((saved_view_state, floor))
    }

    fn prepare_slot_resources(
        &mut self,
        entry: &ViewSlotEntry,
        registry: &mut MultiViewRegistry,
        frame_number: u64,
    ) {
        let resources = &mut self.map_context.world.resources;
        resources.insert(CurrentViewSlot(Some(entry.slot)));
        let drape_slot = match entry.policy {
            CoveringPolicy::Independent => entry.slot,
            CoveringPolicy::StereoReuse(primary) => primary,
        };
        resources.insert(DrapeView(Some(drape_slot)));
        match entry.policy {
            CoveringPolicy::Independent => {
                self.prepare_independent_slot(entry.slot, registry, frame_number)
            }
            CoveringPolicy::StereoReuse(primary) => {
                self.prepare_stereo_reuse_slot(primary, registry, frame_number);
            }
        }
    }

    fn prepare_independent_slot(
        &mut self,
        slot: super::ViewSlotId,
        registry: &mut MultiViewRegistry,
        frame_number: u64,
    ) {
        self.map_context.world.resources.insert(EyeInFrame {
            index: 0,
            frame: frame_number,
        });
        if let Some(prepared) = registry
            .slots
            .get_mut(&slot)
            .and_then(|s| s.prepared_terrain.take())
        {
            self.map_context.world.resources.insert(prepared);
        }
        if let Some(prev) = registry
            .slots
            .get(&slot)
            .and_then(|s| s.stereo_snapshot.as_ref())
        {
            if let Some(shared) = &prev.shared {
                self.map_context.world.resources.insert(shared.clone());
            }
            self.map_context
                .world
                .resources
                .insert(prev.surface_tiles.clone());
            snapshot_lod_history(&mut self.map_context.world);
        } else if let Some(state) = registry.slots.get(&slot) {
            self.map_context
                .world
                .resources
                .insert(state.lod_history.clone());
        }
    }

    fn prepare_stereo_reuse_slot(
        &mut self,
        primary: super::ViewSlotId,
        registry: &MultiViewRegistry,
        frame_number: u64,
    ) {
        let resources = &mut self.map_context.world.resources;
        resources.insert(EyeInFrame {
            index: 1,
            frame: frame_number,
        });
        if let Some(snap) = registry
            .slots
            .get(&primary)
            .and_then(|s| s.stereo_snapshot.as_ref())
        {
            if let Some(shared) = &snap.shared {
                resources.insert(shared.with_frame(frame_number));
            }
            resources.insert(snap.surface_tiles.clone());
            resources.insert(snap.surface_copies.clone());
            resources.insert(snap.terrain_frame.clone());
        }
    }

    fn execute_single_slot(
        &mut self,
        entry: &ViewSlotEntry,
        slot_view: &ViewState,
        registry: &mut MultiViewRegistry,
        frame_number: u64,
        floor: &mut SlotResidencyFloor,
    ) -> Result<bool, MultiViewError> {
        self.map_context.view_state = slot_view.clone();
        self.prepare_slot_resources(entry, registry, frame_number);
        self.map_context.world.resources.insert(floor.clone());
        self.map_context.renderer.resources.render_target.take();

        let slot_state = registry
            .slots
            .get_mut(&entry.slot)
            .ok_or(MultiViewError::UnknownSlot { slot: entry.slot })?;
        swap_slot_resources(
            slot_state,
            &mut self.map_context.renderer,
            &mut self.map_context.world,
        );
        let stage_result = crate::map::runtime::frame::run_stages(
            &mut self.schedule,
            &mut self.map_context,
            |label| {
                label != &RenderStageLabel::Input as &dyn StageLabel
                    && label != &RenderStageLabel::Request as &dyn StageLabel
                    && label != &RenderStageLabel::Cleanup as &dyn StageLabel
            },
        );
        self.map_context.renderer.resources.render_target.take();
        if let Some(slot_state) = registry.slots.get_mut(&entry.slot) {
            swap_slot_resources(
                slot_state,
                &mut self.map_context.renderer,
                &mut self.map_context.world,
            );
        }
        stage_result.map_err(|source| MultiViewError::SlotStage {
            slot: Some(entry.slot),
            source,
        })?;
        let dirty = crate::render::frame_signals::is_dirty(&self.map_context.world);
        if matches!(entry.policy, CoveringPolicy::Independent) {
            if let Some(slot_state) = registry.slots.get_mut(&entry.slot) {
                record_slot_post_render(
                    &self.map_context.style,
                    slot_view,
                    &self.map_context.world,
                    slot_state,
                    floor,
                );
            }
        }
        Ok(dirty)
    }

    fn execute_multi_view(
        &mut self,
        frame: &MultiViewFrame,
        active: &[(ViewSlotEntry, ViewState)],
        registry: &mut MultiViewRegistry,
        frame_number: u64,
    ) -> Result<(), MultiViewError> {
        let (saved_view_state, mut floor) =
            self.begin_multi_view_frame(frame, active, registry, frame_number)?;
        let mut dirty_across_slots = false;
        let mut frame_stats = None;
        for (entry, slot_view) in active {
            let slot_res =
                self.execute_single_slot(entry, slot_view, registry, frame_number, &mut floor);
            frame_stats = Some(crate::render::frame_signals::total_frame_stats(
                &mut self.map_context.world,
                frame_stats.as_ref(),
            ));
            match slot_res {
                Ok(dirty) => dirty_across_slots |= dirty,
                Err(err) => {
                    self.map_context.view_state = saved_view_state;
                    return Err(err);
                }
            }
        }

        self.map_context.world.resources.insert(EyeInFrame {
            index: 0,
            frame: frame_number,
        });
        self.map_context.world.resources.insert(floor);
        let cleanup = crate::map::runtime::frame::run_stages_only(
            &mut self.schedule,
            &mut self.map_context,
            |label| label == &RenderStageLabel::Cleanup as &dyn StageLabel,
        );
        self.map_context.view_state = saved_view_state;
        cleanup.map_err(|source| MultiViewError::SlotStage { slot: None, source })?;
        if active.is_empty() {
            crate::map::runtime::frame::finish_frame(&mut self.map_context);
        } else {
            let timings = self
                .map_context
                .world
                .resources
                .get_or_init_mut::<crate::schedule::system::timings::FrameTimings>();
            drop(timings.finish_frame());
            if dirty_across_slots {
                crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
            }
        }
        Ok(())
    }
}

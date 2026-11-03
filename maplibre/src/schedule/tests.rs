#![allow(clippy::expect_used)]

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom},
    render::view_state::ViewState,
    schedule::{StageError, StageResult},
    tcs::{system::SystemError, world::World},
    window::PhysicalSize,
};

#[derive(Default)]
struct Record<const ID: u8>;

impl<const ID: u8> crate::schedule::Stage for Record<ID> {
    fn run(&mut self, context: &mut MapContext) -> StageResult {
        context
            .world
            .resources
            .get_or_init_mut::<Vec<u8>>()
            .push(ID);
        if ID == 2 {
            Err(SystemError::InvalidTile {
                coords: WorldTileCoords::default(),
            }
            .into())
        } else {
            Ok(())
        }
    }
}

async fn context() -> MapContext {
    let (_, renderer) = crate::headless::create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    MapContext {
        renderer,
        style: Default::default(),
        world: World::default(),
        view_state: ViewState::new(
            PhysicalSize::new(16, 16).expect("size"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(0.0),
            cgmath::Deg(0.0),
            cgmath::Rad(0.64),
        ),
    }
}

#[tokio::test]
async fn nested_schedules_run_their_stages_in_order() {
    let mut context = context().await;
    let mut inner = crate::schedule::Schedule::default();
    inner
        .add_stage("first", Record::<1>)
        .expect("schedule")
        .add_stage("last", Record::<3>)
        .expect("schedule");
    let mut outer = crate::schedule::Schedule::default();
    outer.add_stage("inner", inner).expect("schedule");
    outer.run_once(&mut context).expect("successful stages");
    outer.run_once(&mut context).expect("another update");
    assert_eq!(
        context.world.resources.get::<Vec<u8>>(),
        Some(&vec![1, 3, 1, 3])
    );
}

#[tokio::test]
async fn stage_times_are_kept_in_run_order_without_frame_bookkeeping() {
    let mut context = context().await;
    let mut schedule = crate::schedule::Schedule::default();
    schedule
        .add_stage("first", Record::<1>)
        .expect("schedule")
        .add_stage("last", Record::<3>)
        .expect("schedule");
    schedule.run_once(&mut context).expect("successful stages");
    let stages = context
        .world
        .resources
        .get_or_init_mut::<crate::tcs::system::timings::FrameTimings>()
        .take_stage_times();
    let names: Vec<&str> = stages.iter().map(|(name, _)| name.as_ref()).collect();
    assert_eq!(names, ["\"first\"", "\"last\""]);
    assert_eq!(
        context.last_frame_stats().frame,
        0,
        "only a frame run finishes a frame"
    );
}

#[tokio::test]
async fn a_failed_stage_still_runs_cleanup_stages_and_finishes_the_frame() {
    let mut context = context().await;
    let mut schedule = crate::schedule::Schedule::default();
    schedule
        .add_stage("first", Record::<1>)
        .expect("schedule")
        .add_stage("failing", Record::<2>)
        .expect("schedule")
        .add_stage("skipped", Record::<3>)
        .expect("schedule")
        .add_stage("cleanup", Record::<4>)
        .expect("schedule")
        .run_after_failure("cleanup")
        .expect("schedule");

    let error = crate::render::frame::run_frame(&mut schedule, &mut context)
        .expect_err("second stage fails");

    assert!(matches!(
        error,
        StageError::System(SystemError::InvalidTile { .. })
    ));
    assert_eq!(
        context.world.resources.get::<Vec<u8>>(),
        Some(&vec![1, 2, 4])
    );
    let finished = context.last_frame_stats();
    assert_eq!(finished.frame, 1);
    assert_eq!(finished.stages.len(), 3);
}

#[tokio::test]
async fn the_frame_after_a_failure_starts_from_the_first_stage() {
    let mut context = context().await;
    let mut schedule = crate::schedule::Schedule::default();
    schedule
        .add_stage("first", Record::<1>)
        .expect("schedule")
        .add_stage("failing", Record::<2>)
        .expect("schedule")
        .add_stage("cleanup", Record::<4>)
        .expect("schedule")
        .run_after_failure("cleanup")
        .expect("schedule");

    schedule
        .run_once(&mut context)
        .expect_err("first frame fails");
    schedule
        .run_once(&mut context)
        .expect_err("second frame fails");

    assert_eq!(
        context.world.resources.get::<Vec<u8>>(),
        Some(&vec![1, 2, 4, 1, 2, 4])
    );
}

#[test]
fn builders_report_missing_and_duplicate_stages() {
    use crate::schedule::{NopStage, Schedule, ScheduleError};

    let mut schedule = Schedule::default();
    schedule.add_stage("only", NopStage).expect("new stage");

    assert_eq!(
        schedule.add_stage("only", NopStage).err(),
        Some(ScheduleError::DuplicateStage("\"only\"".into()))
    );
    assert_eq!(
        schedule.add_stage_after("missing", "new", NopStage).err(),
        Some(ScheduleError::MissingStage("\"missing\"".into()))
    );
    assert_eq!(
        schedule
            .add_system_to_stage("only", |_: &mut MapContext| Ok(()))
            .err(),
        Some(ScheduleError::NotASystemStage("\"only\"".into()))
    );
}

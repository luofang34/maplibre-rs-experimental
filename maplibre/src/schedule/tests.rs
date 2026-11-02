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

crate::multi_stage!(Successful, first: Record<1>, last: Record<3>);
crate::multi_stage!(Failing, first: Record<1>, failure: Record<2>, last: Record<3>);

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
async fn composed_stages_execute_in_order() {
    let mut context = context().await;
    let mut stage = Successful::default();
    crate::schedule::Stage::run(&mut stage, &mut context).expect("successful stages");
    crate::schedule::Stage::run(&mut stage, &mut context).expect("another update");
    assert_eq!(
        context.world.resources.get::<Vec<u8>>(),
        Some(&vec![1, 3, 1, 3])
    );
}

#[tokio::test]
async fn composed_stages_propagate_failure_without_running_later_stages() {
    let mut context = context().await;
    let error = crate::schedule::Stage::run(&mut Failing::default(), &mut context)
        .expect_err("second stage fails");
    assert!(
        matches!(error, StageError::System(SystemError::InvalidTile { coords })
        if coords == WorldTileCoords::default())
    );
    assert_eq!(context.world.resources.get::<Vec<u8>>(), Some(&vec![1, 2]));
}

#[tokio::test]
async fn a_failed_stage_still_runs_cleanup_stages_and_finishes_the_frame() {
    let mut context = context().await;
    let mut schedule = crate::schedule::Schedule::default();
    schedule
        .add_stage("first", Record::<1>)
        .add_stage("failing", Record::<2>)
        .add_stage("skipped", Record::<3>)
        .add_stage("cleanup", Record::<4>)
        .run_after_failure("cleanup");

    let error = schedule
        .run_once(&mut context)
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
        .add_stage("failing", Record::<2>)
        .add_stage("cleanup", Record::<4>)
        .run_after_failure("cleanup");

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

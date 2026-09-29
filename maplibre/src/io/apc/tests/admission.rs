#![allow(clippy::expect_used, clippy::panic)]

use std::error::Error;

use crate::{
    io::{
        apc::CallError,
        scheduler::ScheduleError,
        tile_backpressure::{request_budget, tiles_in_flight, MAX_TILES_IN_FLIGHT},
    },
    projection::globe::camera::GlobeCameraError,
    render::projection::ProjectionStateError,
};

mod fixture;
use fixture::{setup, ExistingContent, Kind};

async fn preserves_budget(kind: Kind) {
    let (admission, mut system, mut context) = setup(kind, false).await;
    let result = system.run(&mut context);
    assert_eq!(admission.attempts.get(), 1);
    assert_eq!(
        tiles_in_flight(&context.world.tiles),
        0,
        "no rejected work is in flight"
    );
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT);
    assert_eq!(
        context
            .world
            .tiles
            .query::<&ExistingContent>(Default::default())
            .expect("existing data")
            .0,
        42
    );
    assert!(result.is_err(), "admission failure reaches the caller");
}

async fn retries(kind: Kind) {
    let (admission, mut system, mut context) = setup(kind, false).await;
    system.run(&mut context).ok();
    assert_eq!(admission.attempts.get(), 1);
    admission.reject.set(false);
    system.run(&mut context).expect("admission recovered");
    assert_eq!(
        admission.attempts.get(),
        2,
        "stationary view retries rejected work"
    );
    assert_eq!(tiles_in_flight(&context.world.tiles), 1);
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT - 1);
    system.run(&mut context).expect("pending work is retained");
    assert_eq!(
        admission.attempts.get(),
        2,
        "admitted work is not requested twice"
    );
}

async fn preserves_admission_cause(kind: Kind) {
    let (_, mut system, mut context) = setup(kind, false).await;
    let error = system
        .run(&mut context)
        .expect_err("scheduler rejected work");
    let call = error
        .source()
        .expect("call cause")
        .downcast_ref::<CallError>()
        .expect("typed call");
    let schedule = call
        .source()
        .expect("schedule cause")
        .downcast_ref::<ScheduleError>()
        .expect("typed schedule");
    let transport = schedule
        .source()
        .expect("transport cause")
        .downcast_ref::<std::io::Error>()
        .expect("typed transport");
    assert_eq!(transport.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(transport.to_string(), "worker unavailable");
    assert!(error
        .to_string()
        .contains(&crate::coords::WorldTileCoords::default().to_string()));
}

async fn preserves_projection_cause(kind: Kind) {
    let (admission, mut system, mut context) = setup(kind, true).await;
    let error = system.run(&mut context).expect_err("invalid camera");
    let projection = error
        .source()
        .expect("projection cause")
        .downcast_ref::<ProjectionStateError>()
        .expect("typed projection");
    assert!(matches!(
        projection
            .source()
            .expect("camera cause")
            .downcast_ref::<GlobeCameraError>(),
        Some(GlobeCameraError::InvalidFieldOfView { .. })
    ));
    assert_eq!(admission.attempts.get(), 0);
    assert_eq!(tiles_in_flight(&context.world.tiles), 0);
}

#[tokio::test]
async fn vector_rejection_preserves_budget() {
    preserves_budget(Kind::Vector).await;
}
#[tokio::test]
async fn raster_rejection_preserves_budget() {
    preserves_budget(Kind::Raster).await;
}
#[tokio::test]
async fn dem_rejection_preserves_budget() {
    preserves_budget(Kind::Dem).await;
}
#[tokio::test]
async fn vector_retries_after_scheduler_recovers() {
    retries(Kind::Vector).await;
}
#[tokio::test]
async fn raster_retries_after_scheduler_recovers() {
    retries(Kind::Raster).await;
}
#[tokio::test]
async fn dem_retries_after_scheduler_recovers() {
    retries(Kind::Dem).await;
}
#[tokio::test]
async fn vector_admission_error_retains_cause() {
    preserves_admission_cause(Kind::Vector).await;
}
#[tokio::test]
async fn raster_admission_error_retains_cause() {
    preserves_admission_cause(Kind::Raster).await;
}
#[tokio::test]
async fn dem_admission_error_retains_cause() {
    preserves_admission_cause(Kind::Dem).await;
}
#[tokio::test]
async fn vector_projection_error_retains_cause() {
    preserves_projection_cause(Kind::Vector).await;
}
#[tokio::test]
async fn raster_projection_error_retains_cause() {
    preserves_projection_cause(Kind::Raster).await;
}
#[tokio::test]
async fn dem_projection_error_retains_cause() {
    preserves_projection_cause(Kind::Dem).await;
}

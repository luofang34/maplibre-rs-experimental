#![allow(clippy::expect_used, clippy::panic)]

use std::error::Error;

use crate::{
    io::tile_backpressure::{request_budget, tiles_in_flight, MAX_TILES_IN_FLIGHT},
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
        tiles_in_flight(&context.world),
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
    assert!(
        result.is_ok(),
        "a rejected tile is logged and does not fail the frame"
    );
    crate::render::frame_signals::finish_frame(&mut context);
    assert!(
        context.needs_redraw(),
        "a rejected tile asks for a later frame so a stationary map retries it"
    );
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
    assert_eq!(tiles_in_flight(&context.world), 1);
    assert_eq!(request_budget(&context.world), MAX_TILES_IN_FLIGHT - 1);
    system.run(&mut context).expect("pending work is retained");
    assert_eq!(
        admission.attempts.get(),
        2,
        "admitted work is not requested twice"
    );
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
    assert_eq!(tiles_in_flight(&context.world), 0);
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

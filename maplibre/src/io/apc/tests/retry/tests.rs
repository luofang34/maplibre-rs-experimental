#![allow(clippy::expect_used, clippy::panic)]

use super::{
    fixture::{Fixture, Kind},
    source::Response,
};
use crate::io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT};

async fn recovers(kind: Kind) {
    let mut test = Fixture::new(kind, false).await;
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 1);
    assert!(!test.loaded());
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
    test.source.set(Response::Image);
    test.frame(999);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        1,
        "backoff prevents a request storm"
    );
    test.frame(1000);
    test.frame(1001);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "only one attempt may be in flight"
    );
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        2,
        "stationary view retries after the deadline"
    );
    assert!(test.loaded(), "recovered source data reaches the map");
    test.frame(60000);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        2,
        "success clears the retry deadline"
    );
}

#[tokio::test]
async fn raster_recovers_from_server_failure_without_camera_motion() {
    recovers(Kind::Raster).await;
}
#[tokio::test]
async fn dem_recovers_from_server_failure_without_camera_motion() {
    recovers(Kind::Dem).await;
}

async fn terminal(kind: Kind, response: Response) {
    let mut test = Fixture::new(kind, false).await;
    test.source.set(response);
    test.frame(0);
    test.receive().await;
    assert!(!test.loaded());
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
    test.source.set(Response::Image);
    for time in [1000, 30000, 60000] {
        test.frame(time);
        test.receive().await;
    }
    assert_eq!(
        test.source.requests(),
        1,
        "terminal failures stay negatively cached"
    );
}
#[tokio::test]
async fn raster_missing_tiles_are_negatively_cached() {
    terminal(Kind::Raster, Response::Status(404)).await;
}
#[tokio::test]
async fn dem_missing_tiles_are_negatively_cached() {
    terminal(Kind::Dem, Response::Status(404)).await;
}
#[tokio::test]
async fn raster_invalid_images_are_negatively_cached() {
    terminal(Kind::Raster, Response::Corrupt).await;
}
#[tokio::test]
async fn dem_invalid_images_are_negatively_cached() {
    terminal(Kind::Dem, Response::Corrupt).await;
}
#[tokio::test]
async fn authorization_failures_do_not_retry_as_network_failures() {
    terminal(Kind::Raster, Response::Status(403)).await;
}

#[tokio::test]
async fn raster_partial_source_success_retains_pixels_and_backs_off() {
    let mut test = Fixture::new(Kind::Raster, true).await;
    let mut time = 0;
    let mut previous_requests = 0;
    for delay in [1000, 2000, 4000, 8000, 16000, 30000, 30000] {
        test.frame(time);
        test.receive().await;
        assert!(test.loaded(), "healthy source stays visible during retries");
        let requests = test.source.requests();
        assert_eq!(
            requests,
            previous_requests + 2,
            "both source requests run at each retry deadline"
        );
        previous_requests = requests;
        test.frame(time + delay - 1);
        test.receive().await;
        assert_eq!(
            test.source.requests(),
            requests,
            "partial success must not reset backoff"
        );
        time += delay;
    }
    test.source.set(Response::Image);
    test.frame(time);
    test.receive().await;
    let requests = test.source.requests();
    test.frame(time + 60000);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        requests,
        "complete success cancels retry"
    );
}

async fn stale_completion(kind: Kind) {
    let mut test = Fixture::new(kind, false).await;
    test.frame(0);
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    assert_eq!(test.kernel.apc().pending(), 1);
    test.kernel.apc().deliver(old);
    test.populate.run(&mut test.context).expect("late reply");
    assert_eq!(
        request_budget(&test.context.world),
        MAX_TILES_IN_FLIGHT - 1,
        "an evicted attempt must not release its replacement's slot"
    );
    test.frame(60000);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "late failure cannot start a duplicate"
    );
    test.source.set(Response::Image);
    test.receive().await;
    assert!(test.loaded());
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
}
#[tokio::test]
async fn raster_late_completion_does_not_finish_a_new_attempt() {
    stale_completion(Kind::Raster).await;
}
#[tokio::test]
async fn dem_late_completion_does_not_finish_a_new_attempt() {
    stale_completion(Kind::Dem).await;
}

async fn disconnected_source_recovers(kind: Kind) {
    let mut test = Fixture::new(kind, false).await;
    test.source.set(Response::Disconnect);
    test.frame(0);
    test.receive().await;
    assert!(!test.loaded());
    test.source.set(Response::Image);
    test.frame(1000);
    test.receive().await;
    assert!(
        test.loaded(),
        "interrupted transport must not be a permanent empty tile"
    );
}
#[tokio::test]
async fn raster_retries_an_interrupted_response() {
    disconnected_source_recovers(Kind::Raster).await;
}
#[tokio::test]
async fn dem_retries_an_interrupted_response() {
    disconnected_source_recovers(Kind::Dem).await;
}

async fn retained_content(kind: Kind) {
    use crate::{
        raster::{AvailableRasterLayerData, RasterLayerData, RasterLayersDataComponent},
        terrain::{dem::DemTile, DemTileComponent, LoadedDem},
    };
    let mut test = Fixture::new(kind, false).await;
    test.frame(0);
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([128, 42, 0, 255]));
    let mut tile = test
        .context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile");
    match kind {
        Kind::Vector => unreachable!("raster/DEM fixture"),
        Kind::Raster => {
            tile.insert(RasterLayersDataComponent {
                layers: vec![RasterLayerData::Available(AvailableRasterLayerData {
                    coords: Default::default(),
                    source: "source".into(),
                    image,
                })],
            });
        }
        Kind::Dem => {
            tile.insert(DemTileComponent::Loaded(LoadedDem::new(
                DemTile::from_image(&image, [256.0, 1.0, 1.0 / 256.0, 32768.0]).expect("DEM"),
            )));
        }
    }
    test.receive().await;
    assert!(test.loaded(), "failure retains existing content");
    test.frame(1000);
    assert!(test.loaded(), "retry admission retains existing content");
    assert_eq!(
        request_budget(&test.context.world),
        MAX_TILES_IN_FLIGHT - 1,
        "retained content must not hide a pending refresh from backpressure"
    );
    test.receive().await;
    assert!(test.loaded());
}
#[tokio::test]
async fn raster_retry_preserves_existing_pixels() {
    retained_content(Kind::Raster).await;
}
#[tokio::test]
async fn dem_retry_preserves_existing_heights() {
    retained_content(Kind::Dem).await;
}

#[tokio::test]
async fn partial_raster_result_keeps_request_capacity_until_final_reply() {
    let mut test = Fixture::new(Kind::Raster, true).await;
    let gate = test.source.block_unstable();
    test.frame(0);
    let kernel = test.kernel.clone();
    let worker = kernel.apc().complete();
    let observer = async {
        gate.entered.notified().await;
        test.populate
            .run(&mut test.context)
            .expect("partial raster result");
        assert!(test.loaded(), "first source supplied imagery");
        assert_eq!(
            request_budget(&test.context.world),
            MAX_TILES_IN_FLIGHT - 1,
            "a source image does not finish the rest of its request"
        );
        gate.release.notify_one();
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(worker, observer);
    })
    .await
    .expect("worker and observer finish");
    test.populate
        .run(&mut test.context)
        .expect("final failed source");
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
    test.source.set(Response::Image);
    test.frame(1000);
    test.receive().await;
    assert_eq!(test.source.requests(), 4);
    assert!(test.loaded());
}

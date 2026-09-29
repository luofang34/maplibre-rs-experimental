#![allow(clippy::expect_used, clippy::panic)]

use std::{error::Error, sync::atomic::Ordering};

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{Input, ProcedureError},
        tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    },
    raster::{DefaultRasterTransferables, RasterLayersDataComponent},
    tcs::world::World,
    vector::{transferables::*, DefaultVectorTransferables},
};

mod fixture;
use fixture::{style, Replies, TileKernel};

async fn fetch(raster: bool, paths: &[&str], replies: Replies) -> Result<(), ProcedureError> {
    let input = Input::TileRequest {
        coords: WorldTileCoords::default(),
        style: style(raster, paths),
    };
    if raster {
        crate::raster::request_system::fetch_raster_apc::<_, DefaultRasterTransferables, _>(
            input,
            replies,
            TileKernel::default(),
        )
        .await
    } else {
        crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>(
            input,
            replies,
            TileKernel::default(),
        )
        .await
    }
}

#[tokio::test]
async fn corrupt_vector_source_finishes_after_reporting_missing_layers() {
    let replies = Replies::default();
    fetch(false, &["bad"], replies.clone())
        .await
        .expect("failure completion delivered");
    let mut messages = std::mem::take(&mut *replies.messages.lock().expect("replies"));
    assert_eq!(messages.len(), 3);
    assert!(messages[0].has_tag(DefaultLayerMissing::message_tag()));
    assert!(messages[1].has_tag(DefaultTileTessellated::message_tag()));
    assert!(messages[2].has_tag(DefaultTileTessellated::message_tag()));
    assert_eq!(
        messages
            .remove(0)
            .into_transferable::<DefaultLayerMissing>()
            .expect("missing layer")
            .layer_name(),
        "roads"
    );
}

#[tokio::test]
async fn corrupt_vector_source_does_not_stop_healthy_later_sources() {
    let replies = Replies::default();
    fetch(false, &["bad", "vector"], replies.clone())
        .await
        .expect("later source processed");
    let messages = std::mem::take(&mut *replies.messages.lock().expect("replies"));
    let mut rendered = Vec::new();
    let mut completions = Vec::new();
    for message in messages {
        if message.has_tag(DefaultLayerTessellated::message_tag()) {
            rendered.push(
                message
                    .into_transferable::<DefaultLayerTessellated>()
                    .expect("geometry")
                    .style_layer_id()
                    .to_owned(),
            );
        } else if message.has_tag(DefaultTileTessellated::message_tag()) {
            completions.push(
                message
                    .into_transferable::<DefaultTileTessellated>()
                    .expect("completion")
                    .pending_symbols(),
            );
        }
    }
    assert_eq!(rendered, ["source-1"]);
    assert_eq!(completions.last(), Some(&false));
}

async fn raster_result(paths: &[&str]) -> World {
    let replies = Replies::default();
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(RasterLayersDataComponent::default());
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    fetch(true, paths, replies.clone())
        .await
        .expect("all raster results delivered");
    for message in std::mem::take(&mut *replies.messages.lock().expect("replies")) {
        crate::raster::populate_world_system::apply_raster_message::<DefaultRasterTransferables>(
            &mut world, message,
        )
        .expect("raster result");
    }
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
    world
}

#[tokio::test]
async fn corrupt_raster_releases_loading_capacity_without_an_image() {
    let world = raster_result(&["bad"]).await;
    assert!(world
        .tiles
        .query::<&RasterLayersDataComponent>(Default::default())
        .expect("raster")
        .is_missing());
}

#[tokio::test]
async fn corrupt_raster_source_does_not_discard_a_healthy_later_image() {
    let world = raster_result(&["bad", "image"]).await;
    let raster = world
        .tiles
        .query::<&RasterLayersDataComponent>(Default::default())
        .expect("raster");
    assert_eq!(raster.layers.len(), 2);
    assert!(raster.has_image());
    assert!(!raster.is_missing());
}

async fn delivery_failure(raster: bool, path: &str) {
    let replies = Replies {
        reject: true,
        ..Default::default()
    };
    let error = fetch(raster, &[path], replies.clone())
        .await
        .expect_err("delivery rejected");
    let ProcedureError::Send(source) = error else {
        panic!("transport failure must stay a transport error")
    };
    let cause = source
        .source()
        .expect("transport cause")
        .downcast_ref::<std::io::Error>()
        .expect("original channel cause");
    assert_eq!(cause.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(
        replies.attempts.load(Ordering::SeqCst),
        1,
        "failed transport must not receive replacement messages"
    );
}

#[tokio::test]
async fn vector_delivery_failure_is_not_reclassified_as_missing_data() {
    delivery_failure(false, "vector").await;
}
#[tokio::test]
async fn raster_delivery_failure_is_not_reclassified_as_missing_data() {
    delivery_failure(true, "image").await;
}

#[cfg(feature = "headless")]
mod covering;

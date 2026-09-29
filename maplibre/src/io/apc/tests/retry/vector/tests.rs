use super::super::{
    fixture::{Fixture, Kind},
    source::Response,
};

#[tokio::test]
async fn stationary_vector_tile_recovers_after_real_http_failure() {
    let mut test = Fixture::new(Kind::Vector, false).await;
    let mut frames = super::render::Frames::new(&mut test);
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 1);
    assert!(!test.loaded());
    test.source.set(Response::Bytes(super::tile()));
    test.frame(999);
    test.receive().await;
    assert_eq!(test.source.requests(), 1, "failure backs off");
    test.frame(1000);
    test.frame(1001);
    assert_eq!(
        test.kernel.apc().pending(),
        1,
        "stationary view admits one retry at its deadline"
    );
    test.receive().await;
    assert_eq!(test.source.requests(), 2);
    assert!(
        test.loaded(),
        "valid MVT reaches the existing tile component"
    );
    super::render::assert_green(&frames.render(&mut test));
    test.frame(60000);
    test.receive().await;
    assert_eq!(test.source.requests(), 2, "success clears the deadline");
}

#[tokio::test]
async fn evicted_vector_completions_cannot_finish_the_new_request() {
    use crate::io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT};
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.frame(0);
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    assert_eq!(test.kernel.apc().pending(), 1);
    test.kernel.apc().deliver(old);
    test.populate.run(&mut test.context).expect("old replies");
    assert_eq!(
        request_budget(&test.context.world),
        MAX_TILES_IN_FLIGHT - 1,
        "late TileTessellated and final outcome cannot finish the new attempt"
    );
    test.frame(60000);
    assert_eq!(test.kernel.apc().pending(), 1, "no duplicate request");
    test.source.set(Response::Bytes(super::tile()));
    test.receive().await;
    assert!(test.loaded());
}

#[tokio::test]
async fn repeated_success_keeps_one_current_bucket_per_style_layer() {
    use crate::{
        io::apc::{AsyncProcedureCall, Input},
        sdf::SymbolLayersDataComponent,
        vector::{DefaultVectorTransferables, VectorLayerBucketComponent},
    };
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.source.set(Response::Bytes(super::tile()));
    test.context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    for _ in 0..2 {
        test.kernel
            .apc()
            .call(
                Input::TileRequest {
                    coords: Default::default(),
                    style: test.context.style.clone(),
                },
                crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>,
            )
            .expect("admitted direct worker");
        test.receive().await;
    }
    assert_eq!(test.source.requests(), 2);
    let component = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("existing component");
    assert_eq!(
        component.layers.len(),
        1,
        "style layer content replaces its own bucket"
    );
}

#[tokio::test]
async fn partial_vector_sources_remain_incomplete_until_the_matching_final_reply() {
    use crate::{
        io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
        vector::VectorLayerBucketComponent,
    };
    let mut test = Fixture::new(Kind::Vector, true).await;
    let mut frames = super::render::Frames::new(&mut test);
    test.source.set_healthy(Response::Bytes(super::tile()));
    test.frame(0);
    test.receive().await;
    assert_eq!(test.source.requests(), 2);
    assert!(
        !test.loaded(),
        "one healthy source does not complete both sources"
    );
    test.source.set(Response::Bytes(super::tile()));
    let gate = test.source.block_unstable();
    test.frame(1000);
    let kernel = test.kernel.clone();
    let work = kernel.apc().complete();
    let observer = async {
        gate.entered.notified().await;
        test.populate
            .run(&mut test.context)
            .expect("partial result");
        assert!(
            !test.loaded(),
            "first source cannot clear the prior missing source"
        );
        assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT - 1);
        gate.release.notify_one();
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(work, observer);
    })
    .await
    .expect("worker completes");
    test.populate.run(&mut test.context).expect("final result");
    assert!(test.loaded());
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT);
    assert_eq!(
        test.context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(Default::default())
            .expect("tile")
            .layers
            .len(),
        2
    );
    super::render::assert_green(&frames.render(&mut test));
}

use super::*;
#[tokio::test]
async fn incomplete_child_keeps_parent_pixels_during_multi_source_recovery() {
    let (mut test, mut frames) = parent().await;
    test.source.set(Response::Status(503));
    test.frame(0);
    test.receive().await;
    assert_green(&frames.render(&mut test));
    assert_eq!(drawn_sources(&test), vec![WorldTileCoords::default()]);
    let child = WorldTileCoords::from((0, 0, 1_u8.into()));
    assert!(
        test.context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(child)
            .expect("child")
            .failed
    );
    test.source.set(Response::Bytes(tile()));
    let gate = test.source.block_unstable();
    test.frame(1000);
    let kernel = test.kernel.clone();
    let work = kernel.apc().complete();
    let observe = async {
        gate.entered.notified().await;
        test.populate
            .run(&mut test.context)
            .expect("healthy source reply");
        assert_green(&frames.render(&mut test));
        assert_eq!(drawn_sources(&test), vec![WorldTileCoords::default()]);
        gate.release.notify_one();
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(work, observe);
    })
    .await
    .expect("source completes");
    test.populate
        .run(&mut test.context)
        .expect("all sources ready");
    frames.render(&mut test);
    assert_green(&frames.render(&mut test));
    assert_eq!(
        drawn_sources(&test),
        vec![child],
        "complete uploaded child replaces the parent"
    );
}

//! Delayed successful payloads cannot attach to a newer request at the same coordinates.
use super::super::{
    fixture::{Fixture, Kind, TestEnvironment},
    source::Response,
};
use crate::{
    io::{
        apc::{AsyncProcedureCall, Input},
        tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    },
    sdf::SymbolLayersDataComponent,
    tcs::system::System,
    vector::{DefaultVectorTransferables, SymbolLayerTessellated, VectorLayerBucketComponent},
};

async fn stale_success(tracked: bool) {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style.layers.push(
        serde_json::from_value(serde_json::json!({
            "id":"label","type":"symbol","source":"source","source-layer":"land",
            "layout":{"text-field":"X"},"paint":{"text-color":"#ffffff"}
        }))
        .expect("symbol style"),
    );
    test.source.set(Response::Bytes(super::tile()));
    if tracked {
        test.frame(0);
    } else {
        test.context
            .world
            .tiles
            .spawn_mut(Default::default())
            .expect("tile")
            .insert(VectorLayerBucketComponent::default())
            .insert(SymbolLayersDataComponent::default());
        test.kernel
            .apc()
            .call(
                Input::TileRequest {
                    coords: Default::default(),
                    style: test.context.style.clone(),
                },
                crate::vector::request_system::fetch_vector_apc::<_, DefaultVectorTransferables, _>,
            )
            .expect("direct request");
    }
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    assert!(
        old.iter().any(|message| message
            .has_tag(crate::vector::transferables::DefaultSymbolLayerTessellated::message_tag())),
        "worker really produced symbols"
    );
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    test.kernel.apc().deliver(old);
    test.populate
        .run(&mut test.context)
        .expect("old vector replies");
    let mut symbols = crate::sdf::populate_world_system::PopulateWorldSystem::<
        TestEnvironment,
        DefaultVectorTransferables,
    >::new(&test.kernel);
    symbols.run(&mut test.context).expect("old symbol replies");
    assert_pending(&test);
    test.receive().await;
    symbols.run(&mut test.context).expect("new symbol replies");
    assert!(test.loaded());
    assert!(!test
        .context
        .world
        .tiles
        .query::<&SymbolLayersDataComponent>(Default::default())
        .expect("new symbols")
        .layers
        .is_empty());
}

fn assert_pending(test: &Fixture) {
    let component = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("new request");
    assert!(
        !component.done && component.layers.is_empty(),
        "stale geometry and completion are rejected"
    );
    assert!(test
        .context
        .world
        .tiles
        .query::<&SymbolLayersDataComponent>(Default::default())
        .expect("symbols")
        .layers
        .is_empty());
    assert_eq!(
        test.context
            .world
            .tiles
            .geometry_index
            .tile_bytes(Default::default()),
        0,
        "old query geometry is rejected"
    );
    assert_eq!(request_budget(&test.context.world), MAX_TILES_IN_FLIGHT - 1);
}

mod tests;

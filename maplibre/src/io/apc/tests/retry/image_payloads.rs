//! Image workers retain the request that owns each result, including unsuccessful results.

use image::ImageEncoder;

use super::{
    fixture::{Fixture, Kind},
    source::Response,
};
use crate::{
    io::apc::Input,
    raster::{RasterLayerData, RasterLayersDataComponent},
    terrain::DemTileComponent,
};

fn png(value: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[128, value, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .expect("PNG");
    bytes
}

async fn stale_success(kind: Kind) {
    let mut test = Fixture::new(kind, false).await;
    test.source.set(Response::Bytes(png(200)));
    test.frame(0);
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    test.context.world.tiles.remove(Default::default());
    test.source.set(Response::Status(503));
    test.frame(1);
    test.receive().await;
    assert!(!test.loaded());
    test.source.set(Response::Bytes(png(100)));
    test.frame(1001);
    test.receive().await;
    assert_value(&test, kind, 100);
    test.kernel.apc().deliver(old);
    test.populate
        .run(&mut test.context)
        .expect("late successful worker");
    assert_value(&test, kind, 100);
}

async fn stale_missing(kind: Kind) {
    for response in [
        Response::Status(503),
        Response::Status(404),
        Response::Corrupt,
    ] {
        for recovered in [false, true] {
            missing_result(kind, response.clone(), recovered).await;
        }
    }
}

async fn missing_result(kind: Kind, response: Response, recovered: bool) {
    let mut test = Fixture::new(kind, false).await;
    test.source.set(response);
    test.frame(0);
    test.kernel.apc().complete().await;
    let old = test.kernel.apc().take_replies();
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    if recovered {
        test.source.set(Response::Bytes(png(100)));
        test.receive().await;
        assert_value(&test, kind, 100);
    }
    test.kernel.apc().deliver(old);
    test.populate
        .run(&mut test.context)
        .expect("late missing worker");
    if recovered {
        assert_value(&test, kind, 100);
    } else {
        assert_pending(&test, kind);
        test.source.set(Response::Bytes(png(100)));
        test.receive().await;
        assert_value(&test, kind, 100);
    }
}

async fn legacy(kind: Kind) {
    let mut test = Fixture::new(kind, false).await;
    test.source.set(Response::Bytes(png(200)));
    test.frame(0);
    let requests: Vec<_> = test
        .kernel
        .apc()
        .take_requests()
        .into_iter()
        .map(|(input, procedure)| {
            let (coords, style, _) = input.into_tile_request();
            (Input::TileRequest { coords, style }, procedure)
        })
        .collect();
    test.context.world.tiles.remove(Default::default());
    let mut tile = test
        .context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("legacy tile");
    match kind {
        Kind::Raster => {
            tile.insert(RasterLayersDataComponent::default());
        }
        Kind::Dem => {
            tile.insert(DemTileComponent::Pending);
        }
        Kind::Vector => panic!("image request kind"),
    }
    test.kernel.apc().run_requests(requests.clone()).await;
    test.populate
        .run(&mut test.context)
        .expect("valid untracked worker");
    assert_value(&test, kind, 200);
    test.context.world.tiles.remove(Default::default());
    test.frame(1);
    test.kernel.apc().run_requests(requests).await;
    test.populate
        .run(&mut test.context)
        .expect("untracked worker during tracked request");
    assert_pending(&test, kind);
    test.source.set(Response::Bytes(png(100)));
    test.receive().await;
    assert_value(&test, kind, 100);
}

fn assert_pending(test: &Fixture, kind: Kind) {
    match kind {
        Kind::Raster => assert!(
            test.context
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(Default::default())
                .expect("raster request")
                .layers
                .is_empty(),
            "stale Missing/pixels cannot complete a pending raster"
        ),
        Kind::Dem => assert!(
            matches!(
                test.context
                    .world
                    .tiles
                    .query::<&DemTileComponent>(Default::default()),
                Some(DemTileComponent::Pending)
            ),
            "stale Missing/pixels cannot complete a pending DEM"
        ),
        Kind::Vector => panic!("image request kind"),
    }
    assert_eq!(
        crate::io::tile_backpressure::request_budget(&test.context.world),
        crate::io::tile_backpressure::MAX_TILES_IN_FLIGHT - 1
    );
}

fn assert_value(test: &Fixture, kind: Kind, value: u8) {
    match kind {
        Kind::Raster => {
            let layers = &test
                .context
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(Default::default())
                .expect("raster request")
                .layers;
            assert_eq!(layers.len(), 1);
            let RasterLayerData::Available(layer) = &layers[0] else {
                panic!("decoded pixels");
            };
            assert_eq!(
                layer.image.as_raw(),
                &[128, value, 0, 255],
                "current raster pixels stay authoritative"
            );
        }
        Kind::Dem => {
            let Some(DemTileComponent::Loaded(dem)) = test
                .context
                .world
                .tiles
                .query::<&DemTileComponent>(Default::default())
            else {
                panic!("decoded elevation");
            };
            assert_eq!(
                dem.tile.get(0, 0),
                f64::from(value),
                "current DEM elevation stays authoritative"
            );
        }
        Kind::Vector => panic!("image request kind"),
    }
}

mod tests;

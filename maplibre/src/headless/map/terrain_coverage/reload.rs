//! Same-coordinate DEM replacement through the headless and worker ingestion paths.

use super::{prepared_map, target, HeadlessMap, Rgba, RgbaImage, WorldTileCoords};
use crate::{
    headless::environment::HeadlessEnvironment,
    io::apc::{tests::reply_context, Context},
    render::eventually::Eventually,
    tcs::system::System,
    terrain::{
        dem::gpu_readback::{pipeline, sample_gpu_blocking},
        populate_world_system::PopulateWorldSystem,
        resources::TerrainResources,
        DefaultDemTransferables, DefaultLayerDem, DemTileComponent, LayerDem,
    },
};

#[derive(Clone, Copy)]
enum Delivery {
    Headless,
    Worker,
}

fn image(height: u16) -> RgbaImage {
    let encoded = 32768 + height;
    RgbaImage::from_pixel(
        256,
        256,
        Rgba([(encoded >> 8) as u8, encoded as u8, 0, 255]),
    )
}

fn ingest(map: &mut HeadlessMap, delivery: Delivery, coords: WorldTileCoords, height: u16) {
    match delivery {
        Delivery::Headless => map
            .load_dem_tiles(vec![(coords, image(height))])
            .expect("headless DEM ingestion"),
        Delivery::Worker => {
            if map
                .map_context
                .world
                .tiles
                .query::<&DemTileComponent>(coords)
                .is_none()
            {
                map.map_context
                    .world
                    .tiles
                    .spawn_mut(coords)
                    .expect("requested tile")
                    .insert(DemTileComponent::Pending);
            }
            reply_context(map.kernel.apc())
                .send_back(DefaultLayerDem::build_from(coords, image(height)))
                .expect("worker result");
            PopulateWorldSystem::<HeadlessEnvironment, DefaultDemTransferables>::new(&map.kernel)
                .run(&mut map.map_context)
                .expect("worker DEM ingestion");
            map.run_frame().expect("upload and render");
        }
    }
}

fn assert_uploaded(map: &HeadlessMap, coords: WorldTileCoords, expected: f64) {
    let world = map.world();
    let Some(DemTileComponent::Loaded(dem)) = world.tiles.query::<&DemTileComponent>(coords) else {
        panic!("decoded DEM");
    };
    assert_eq!(dem.tile.elevation_at_tile_coords(2048.0, 2048.0), expected);
    let Some(Eventually::Initialized(resources)) =
        world.resources.get::<Eventually<TerrainResources>>()
    else {
        panic!("terrain resources");
    };
    assert!(resources.has_dem_texture(coords));
    let pipeline = pipeline(map.device());
    let heights = sample_gpu_blocking(
        map.device(),
        map.queue(),
        &pipeline,
        &dem.tile,
        resources.dem_texture(Some(coords)),
    );
    for (index, actual) in heights.into_iter().enumerate() {
        let x = (index % 9) as f64 * 512.0;
        let y = (index / 9) as f64 * 512.0;
        let expected = dem.tile.elevation_at_tile_coords(x, y);
        assert!(
            (f64::from(actual) - expected).abs() < 0.001,
            "DEM {coords} at ({x}, {y}): GPU {actual}, CPU {expected}"
        );
    }
}

async fn replace(delivery: Delivery, neighbour: bool) {
    let mut map = prepared_map(true).await;
    let coords = target();
    let east = WorldTileCoords {
        x: coords.x + 1,
        ..coords
    };
    if neighbour {
        ingest(&mut map, delivery, east, 200);
    }
    assert_uploaded(&map, coords, 0.0);
    ingest(&mut map, delivery, coords, 100);
    assert_uploaded(&map, coords, 100.0);
    if neighbour {
        assert_uploaded(&map, east, 200.0);
    }
}

async fn evict_and_reload(delivery: Delivery, drop_gpu: bool) {
    let mut map = prepared_map(true).await;
    assert!(map.map_context.world.tiles.remove(target()));
    if drop_gpu {
        let Some(Eventually::Initialized(resources)) = map
            .map_context
            .world
            .resources
            .get_mut::<Eventually<TerrainResources>>()
        else {
            panic!("terrain resources");
        };
        resources.drop_dem(target());
    }
    ingest(&mut map, delivery, target(), 100);
    assert_uploaded(&map, target(), 100.0);
}

mod tests;

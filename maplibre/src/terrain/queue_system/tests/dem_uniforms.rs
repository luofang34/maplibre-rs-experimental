#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    coords::WorldTileCoords,
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    hillshade::{
        resources::{ColorReliefUniforms, DemLayerKind},
        HillshadePlugin, HillshadeResources,
    },
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{eventually::Eventually, RenderPlugin},
    tcs::world::World,
    terrain::{
        drape_cache::{fingerprint, DrapeCache, DrapeState},
        drape_targets::{ShapeSpec, TargetSpec},
        queue_system::loaded_content,
    },
};

#[tokio::test]
async fn written_dem_uniforms_invalidate_only_specs_using_that_layer() {
    let (map, resources) = resources().await;
    let mut world = World::default();
    world.resources.insert(Eventually::Initialized(resources));
    write(&mut world, &map, "changed", 0.5);
    write(&mut world, &map, "neighbor", 0.5);
    let specs = [spec(0, "changed"), spec(1, "neighbor")];
    let keys = keys_for(&specs, &world);
    let mut cache = DrapeCache::<u8>::default();
    for (target, key) in specs.iter().zip(keys) {
        assert_eq!(
            cache.acquire(target.coords, key, true, || 7),
            DrapeState::New
        );
    }

    write(&mut world, &map, "changed", 0.0);
    let changed = keys_for(&specs, &world);
    assert_ne!(changed[0], keys[0]);
    assert_eq!(changed[1], keys[1]);
    for ((target, key), expected) in specs
        .iter()
        .zip(changed)
        .zip([DrapeState::Changed, DrapeState::Unchanged])
    {
        assert_eq!(cache.acquire(target.coords, key, true, || 8), expected);
    }
    write(&mut world, &map, "changed", 0.0);
    for (target, key) in specs.iter().zip(keys_for(&specs, &world)) {
        assert_eq!(
            cache.acquire(target.coords, key, true, || 9),
            DrapeState::Unchanged
        );
    }
}

fn keys_for(specs: &[TargetSpec; 2], world: &World) -> [u64; 2] {
    specs
        .each_ref()
        .map(|spec| fingerprint(spec, &loaded_content(world), wgpu::Color::BLACK, 12.0))
}

fn spec(x: i32, layer: &str) -> TargetSpec {
    let coords = WorldTileCoords::from((x, 0, 2_u8.into()));
    TargetSpec {
        coords,
        shapes: vec![ShapeSpec {
            source: coords,
            vector_layers: Vec::new(),
            raster_layers: vec![(layer.into(), 0, true)],
        }],
    }
}

fn write(world: &mut World, map: &HeadlessMap, layer: &str, opacity: f32) {
    let Some(Eventually::Initialized(resources)) =
        world.resources.get_mut::<Eventually<HillshadeResources>>()
    else {
        panic!("DEM resources");
    };
    let uniforms = ColorReliefUniforms::new([0.0; 4], opacity, &[(0.0, [1.0; 4])]);
    resources.write_layer(
        map.device(),
        map.queue(),
        layer,
        DemLayerKind::ColorRelief,
        bytemuck::bytes_of(&uniforms),
    );
}

async fn resources() -> (HeadlessMap, HillshadeResources) {
    let (kernel, renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[]}))
            .expect("style"),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    map.run_frame().expect("shader resources");
    let Some(Eventually::Initialized(resources)) = map
        .world()
        .resources
        .get::<Eventually<HillshadeResources>>()
    else {
        panic!("shader resources");
    };
    let resources = HillshadeResources::new(
        resources.pipeline(DemLayerKind::Hillshade).clone(),
        resources.pipeline(DemLayerKind::ColorRelief).clone(),
    );
    (map, resources)
}

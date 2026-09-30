use maplibre::{
    map::Map,
    raster::{
        DefaultRasterTransferables, RasterLayerData, RasterLayersDataComponent, RasterPlugin,
    },
    render::RenderPlugin,
    terrain::{DefaultDemTransferables, DemTileComponent, TerrainPlugin},
};
use maplibre_winit::WinitMapWindowConfig;

use super::{fixture, frame, initialize, reset, Environment, Source};

pub(super) async fn check(config: WinitMapWindowConfig<()>, source: &Source) {
    for dem in [false, true] {
        if !dem && std::env::args().any(|arg| arg == "--dem-only") {
            continue;
        }
        let mut map = create_map(config.clone(), &source.url, dem);
        initialize(&mut map).await;
        source.image(200);
        frame(&mut map, 0);
        let delayed = map.kernel().apc().take_one().execute().await;
        reset(&mut map).await;
        source.image(100);
        let current = map.kernel().apc().take_one();
        map.kernel().apc().deliver(current.execute().await);
        frame(&mut map, 1);
        assert_value(&map, dem);
        map.kernel().apc().deliver(delayed);
        frame(&mut map, 2);
        assert_value(&map, dem);

        reset(&mut map).await;
        source.unavailable();
        let delayed = map.kernel().apc().take_one().execute().await;
        reset(&mut map).await;
        map.kernel().apc().deliver(delayed);
        frame(&mut map, 1);
        let world = &map.context().expect("new world").world;
        if dem {
            assert!(
                matches!(
                    world.tiles.query::<&DemTileComponent>(Default::default()),
                    Some(DemTileComponent::Pending)
                ),
                "old DEM error cannot finish a new world request"
            );
        } else {
            assert!(
                world
                    .tiles
                    .query::<&RasterLayersDataComponent>(Default::default())
                    .expect("raster request")
                    .layers
                    .is_empty(),
                "old raster error cannot finish a new world request"
            );
        }
        source.image(100);
        let current = map.kernel().apc().take_one();
        map.kernel().apc().deliver(current.execute().await);
        frame(&mut map, 2);
        assert_value(&map, dem);
    }
}

fn create_map(config: WinitMapWindowConfig<()>, url: &str, dem: bool) -> Map<Environment> {
    let mut json = serde_json::json!({
        "version":8,"center":[0,0],"zoom":0,
        "sources":{"source":{"type":if dem { "raster-dem" } else { "raster" },"tiles":[format!("{url}/{{z}}/{{x}}/{{y}}")],"maxzoom":0,"encoding":"terrarium"}},
        "layers":[{"id":"image","source":"source","type":"raster"}]
    });
    let mut plugins: Vec<Box<dyn maplibre::plugin::Plugin<Environment>>> = vec![
        Box::new(RenderPlugin),
        Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
    ];
    if dem {
        json["terrain"] = serde_json::json!({"source":"source"});
        json["layers"] = serde_json::json!([]);
        plugins.push(Box::new(TerrainPlugin::<DefaultDemTransferables>::default()));
    }
    fixture::create_map(
        config,
        serde_json::from_value(json).expect("image style"),
        plugins,
    )
}

fn assert_value(map: &Map<Environment>, dem: bool) {
    let world = &map.context().expect("ready image map").world;
    if dem {
        let Some(DemTileComponent::Loaded(loaded)) =
            world.tiles.query::<&DemTileComponent>(Default::default())
        else {
            panic!("valid elevation");
        };
        assert_eq!(
            loaded.tile.get(0, 0),
            100.0,
            "old DEM cannot replace current elevation after Map.reset"
        );
    } else {
        let layers = &world
            .tiles
            .query::<&RasterLayersDataComponent>(Default::default())
            .expect("raster request")
            .layers;
        assert_eq!(layers.len(), 1);
        let RasterLayerData::Available(layer) = &layers[0] else {
            panic!("valid pixels");
        };
        assert_eq!(
            layer.image.as_raw(),
            &[128, 100, 0, 255],
            "old raster cannot replace current pixels after Map.reset"
        );
        let Some(maplibre::render::eventually::Eventually::Initialized(resources)) =
            world
                .resources
                .get::<maplibre::render::eventually::Eventually<
                    maplibre::raster::resource::RasterResources,
                >>()
        else {
            panic!("raster GPU resources");
        };
        assert!(
            resources
                .get_bound_texture(
                    &maplibre::raster::RasterSourceId::new(Some("source".into())),
                    &Default::default()
                )
                .is_some(),
            "current image reaches GPU texture binding"
        );
    }
}

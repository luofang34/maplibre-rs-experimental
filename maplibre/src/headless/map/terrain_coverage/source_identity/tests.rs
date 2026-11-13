use super::*;
use crate::{render::eventually::Eventually, terrain::resources::TerrainResources};

#[test]
fn unrelated_raster_source_cannot_complete_missing_raster_coverage() {
    crate::headless::map::test_async::run_async(async {
        assert_pending_b(false).await;
    })
}

#[test]
fn imagery_source_cannot_complete_missing_color_relief_coverage() {
    crate::headless::map::test_async::run_async(async {
        assert_pending_b(true).await;
    })
}

async fn assert_pending_b(relief_b: bool) {
    let mut map = empty_map(relief_b).await;
    let attempt = prepare_pending(&mut map);
    let gate = Arc::new(SourceGate::default());
    let mut request = source_request(&map, relief_b, Some(gate.clone()), attempt);
    match futures::future::select(std::pin::pin!(gate.entered.notified()), &mut request).await {
        futures::future::Either::Left(((), _)) => {}
        futures::future::Either::Right((result, _)) => {
            panic!("request finished before B's gate: {result:?}")
        }
    }
    apply_worker_sources(&mut map);
    let pixels = read_blocking(
        &map,
        if relief_b {
            "identity-missing-relief"
        } else {
            "identity-missing-raster"
        },
    );
    let Some(Eventually::Initialized(terrain)) =
        map.world().resources.get::<Eventually<TerrainResources>>()
    else {
        panic!("terrain")
    };
    assert!(
        terrain.drape_texture(target()).is_none(),
        "source A imagery must not publish a complete drape while source B has no pixels"
    );
    assert_color(&pixels, [16, 16, 16, 255]);
    gate.resume.notify_one();
    request.await.expect("B completed");
}

#[test]
fn native_worker_preserves_both_sources_at_the_same_coordinates() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(true).await;
        deliver_worker_sources(&mut map, true).await;
        let layers = &map
            .world()
            .tiles
            .query::<&RasterLayersDataComponent>(target())
            .expect("raster component")
            .layers;
        assert_eq!(
            layers.len(),
            2,
            "one source response must not overwrite another source"
        );
    })
}

#[test]
fn raster_and_relief_draw_their_own_source_pixels() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(true).await;
        deliver_worker_sources(&mut map, true).await;
        assert_color(
            &read_blocking(&map, "identity-mixed-worker"),
            [0, 128, 128, 255],
        );
    })
}

#[test]
fn headless_separate_source_arrivals_preserve_both_images() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(true).await;
        for (source, pixel) in [("a", [0, 255, 0, 255]), ("b", [128, 100, 0, 255])] {
            map.render_sources(
                ProcessedLayers::default(),
                vec![AvailableRasterLayerData {
                    coords: target(),
                    source: source.into(),
                    image: RgbaImage::from_pixel(256, 256, Rgba(pixel)),
                }],
            )
            .expect("source arrival");
        }
        assert_color(
            &read_blocking(&map, "identity-mixed-headless"),
            [0, 128, 128, 255],
        );
    })
}

#[test]
fn screen_raster_and_relief_keep_independent_bindings() {
    crate::headless::map::test_async::run_async(async {
        let mut map = source_map(true, false).await;
        deliver_worker_sources(&mut map, true).await;
        assert_color(
            &read_blocking(&map, "identity-screen-mixed"),
            [0, 128, 128, 255],
        );
    })
}

#[test]
fn each_source_keeps_its_parent_until_its_own_children_arrive() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(true).await;
        deliver_worker_sources(&mut map, true).await;
        assert_color(
            &read_blocking(&map, "identity-parent-pair"),
            [0, 128, 128, 255],
        );
        for child in target().get_children() {
            map.render_sources(
                ProcessedLayers::default(),
                vec![source_tile("a", child, [0, 255, 0, 255])],
            )
            .expect("imagery child");
        }
        assert_color(
            &read_blocking(&map, "identity-a-children-b-parent"),
            [0, 128, 128, 255],
        );
        for (index, child) in target().get_children().into_iter().enumerate() {
            map.render_sources(
                ProcessedLayers::default(),
                vec![source_tile("b", child, [128, 0, 0, 255])],
            )
            .expect("relief child");
            assert_color(
                &read_blocking(&map, &format!("identity-b-child-{index}")),
                if index == 3 {
                    [128, 128, 0, 255]
                } else {
                    [0, 128, 128, 255]
                },
            );
        }
    })
}

#[test]
fn refreshing_one_source_preserves_another_sources_binding() {
    crate::headless::map::test_async::run_async(async {
        use crate::raster::resource::RasterResources;
        let mut map = empty_map(true).await;
        deliver_worker_sources(&mut map, true).await;
        let revision = |map: &HeadlessMap| {
            let Some(Eventually::Initialized(resources)) =
                map.world().resources.get::<Eventually<RasterResources>>()
            else {
                panic!("raster")
            };
            resources.texture_revision("b", target()).expect("B upload")
        };
        let before = revision(&map);
        map.render_sources(
            ProcessedLayers::default(),
            vec![source_tile("a", target(), [255, 0, 0, 255])],
        )
        .expect("refresh A");
        assert_eq!(revision(&map), before, "A must not discard or upload B");
        assert_color(
            &read_blocking(&map, "identity-refresh-a"),
            [128, 0, 128, 255],
        );
    })
}

fn source_tile(source: &str, coords: WorldTileCoords, pixel: [u8; 4]) -> AvailableRasterLayerData {
    AvailableRasterLayerData {
        coords,
        source: source.into(),
        image: RgbaImage::from_pixel(256, 256, Rgba(pixel)),
    }
}

#[test]
fn style_source_changes_and_layer_removal_do_not_reuse_stale_bindings() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(false).await;
        deliver_worker_sources(&mut map, false).await;
        assert_color(
            &read_blocking(&map, "identity-style-before"),
            [0, 0, 255, 255],
        );
        map.map_context.style.layers[2].source = Some("a".into());
        map.run_frame().expect("change source");
        assert_color(
            &read_blocking(&map, "identity-style-source-a"),
            [0, 255, 0, 255],
        );
        let mut layer = map.map_context.style.layers.pop().expect("B layer");
        map.run_frame().expect("remove B layer");
        layer.source = Some("b".into());
        map.map_context.style.layers.push(layer);
        map.run_frame().expect("restore B layer");
        assert_color(
            &read_blocking(&map, "identity-style-restored"),
            [0, 0, 255, 255],
        );
    })
}

#[test]
fn coordinate_eviction_reuploads_each_sources_own_pixels() {
    crate::headless::map::test_async::run_async(async {
        use crate::raster::resource::RasterResources;
        let mut map = empty_map(true).await;
        deliver_worker_sources(&mut map, true).await;
        let Some(Eventually::Initialized(resources)) = map
            .map_context
            .world
            .resources
            .get_mut::<Eventually<RasterResources>>()
        else {
            panic!("raster")
        };
        resources.remove_texture(target());
        assert!(resources
            .get_bound_texture(&"a".into(), &target())
            .is_none());
        assert!(resources
            .get_bound_texture(&"b".into(), &target())
            .is_none());
        map.run_frame().expect("reupload source pair");
        assert_color(
            &read_blocking(&map, "identity-coordinate-eviction"),
            [0, 128, 128, 255],
        );
    })
}

#[test]
fn unnamed_image_is_not_a_wildcard_for_named_style_sources() {
    crate::headless::map::test_async::run_async(async {
        let mut map = empty_map(false).await;
        let mut unnamed = source_tile("", target(), [255, 0, 0, 255]);
        unnamed.source = Default::default();
        map.render_sources(
            ProcessedLayers::default(),
            vec![unnamed, source_tile("a", target(), [0, 255, 0, 255])],
        )
        .expect("unnamed and A images");
        let Some(Eventually::Initialized(terrain)) =
            map.world().resources.get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain")
        };
        assert!(terrain.drape_texture(target()).is_none());
        assert_color(
            &read_blocking(&map, "identity-unnamed-pending-b"),
            [16, 16, 16, 255],
        );
    })
}

#[test]
fn screen_named_and_unnamed_sources_keep_independent_parent_fallback() {
    crate::headless::map::test_async::run_async(async {
        assert_default_parent(false, false).await;
    })
}

#[test]
fn terrain_named_and_unnamed_sources_keep_independent_parent_fallback() {
    crate::headless::map::test_async::run_async(async {
        assert_default_parent(true, false).await;
    })
}

#[test]
fn screen_source_without_tile_urls_uses_the_default_sources_parent() {
    crate::headless::map::test_async::run_async(async {
        assert_default_parent(false, true).await;
    })
}

#[test]
fn terrain_source_without_tile_urls_uses_the_default_sources_parent() {
    crate::headless::map::test_async::run_async(async {
        assert_default_parent(true, true).await;
    })
}

async fn assert_default_parent(terrain: bool, missing_urls: bool) {
    let mut map = source_map(false, terrain).await;
    if missing_urls {
        let Some(crate::style::source::Source::Raster(source)) =
            map.map_context.style.sources.get_mut("b")
        else {
            panic!("B raster")
        };
        source.tiles = None;
    } else {
        map.map_context.style.layers[2].source = None;
    }
    let required_default = map
        .required_raster_source_tile_coords(&Default::default())
        .expect("default covering");
    assert!(!required_default.is_empty());
    assert_ne!(
        required_default,
        map.required_raster_tile_coords("a")
            .expect("named covering")
    );
    if missing_urls {
        assert_eq!(
            map.required_raster_tile_coords("b")
                .expect("fallback alias"),
            required_default
        );
    }
    let mut fallback = source_tile("", target().get_parent().expect("parent"), [0, 0, 255, 255]);
    fallback.source = Default::default();
    map.render_sources(
        ProcessedLayers::default(),
        vec![source_tile("a", target(), [0, 255, 0, 255]), fallback],
    )
    .expect("named and default images");
    assert_color(
        &read_blocking(
            &map,
            &format!("identity-default-parent-terrain-{terrain}-url-less-{missing_urls}"),
        ),
        [0, 0, 255, 255],
    );
}

use super::*;
use crate::{coords::Zoom, raster::resource::RasterResources, render::eventually::Eventually};

#[tokio::test]
async fn relief_keeps_complete_parent_until_all_children_arrive() {
    batched_children(true).await;
}

#[tokio::test]
async fn raster_keeps_complete_parent_until_all_children_arrive() {
    batched_children(false).await;
}

#[tokio::test]
async fn same_coordinate_raster_reupload_refreshes_the_cached_drape() {
    let mut map = prepared_map(false).await;
    let Some(Eventually::Initialized(raster)) = map
        .map_context
        .world
        .resources
        .get_mut::<Eventually<RasterResources>>()
    else {
        panic!("raster resources");
    };
    raster.remove_texture(target());
    map.render_sources(
        ProcessedLayers::default(),
        vec![tile(target(), false, true)],
    )
    .expect("replacement upload");
    assert_color(&read_blocking(&map, "raster-reuploaded"), [0, 0, 255, 255]);
}

#[tokio::test]
async fn same_coordinate_source_replacement_refreshes_the_cached_drape() {
    let mut map = prepared_map(true).await;
    map.render_sources(ProcessedLayers::default(), vec![tile(target(), true, true)])
        .expect("replacement source");
    assert_color(&read_blocking(&map, "relief-replaced"), [8, 8, 136, 255]);
}

async fn batched_children(relief: bool) {
    let name = if relief { "relief" } else { "raster" };
    let mut map = prepared_map(relief).await;
    let before = read_blocking(&map, &format!("{name}-parent"));
    let parent_color = if relief {
        [8, 136, 8, 255]
    } else {
        [0, 255, 0, 255]
    };
    let child_color = if relief {
        [8, 8, 136, 255]
    } else {
        [0, 0, 255, 255]
    };
    assert_color(&before, parent_color);
    map.map_context.view_state.zoom_to(Zoom::new(12.15));
    for (arrival, quadrant) in [0, 2, 1, 3].into_iter().enumerate() {
        let child = target().get_children()[quadrant];
        map.render_sources(ProcessedLayers::default(), vec![tile(child, relief, true)])
            .expect("child arrival frame");
        let bytes = read_blocking(&map, &format!("{name}-children-{}", arrival + 1));
        assert_color(
            &bytes,
            if arrival == 3 {
                child_color
            } else {
                parent_color
            },
        );
    }
}

#[tokio::test]
async fn losing_source_coverage_preserves_cached_terrain_texture_until_reload() {
    let mut map = prepared_map(true).await;
    let children = target().get_children();
    map.render_sources(
        ProcessedLayers::default(),
        children.into_iter().map(|c| tile(c, true, true)).collect(),
    )
    .expect("complete child frame");
    assert_color(&read_blocking(&map, "relief-complete"), [8, 8, 136, 255]);
    for coords in [target(), children[0]] {
        assert!(map.map_context.world.tiles.remove(coords));
        let Some(Eventually::Initialized(raster)) = map
            .map_context
            .world
            .resources
            .get_mut::<Eventually<RasterResources>>()
        else {
            panic!("raster resources");
        };
        raster.remove_texture(coords);
    }
    map.run_frame().expect("eviction frame");
    assert_color(&read_blocking(&map, "relief-evicted"), [8, 8, 136, 255]);
    map.render_sources(
        ProcessedLayers::default(),
        vec![tile(children[0], true, true)],
    )
    .expect("reload frame");
    assert_color(&read_blocking(&map, "relief-reloaded"), [8, 8, 136, 255]);
}

#[tokio::test]
async fn incomplete_uploads_cannot_replace_a_complete_drape() {
    let mut map = prepared_map(true).await;
    let children = target().get_children();
    map.map_context
        .world
        .resources
        .get_mut::<faults::Faults>()
        .expect("controls")
        .missing_upload = Some(children[2]);
    map.render_sources(
        ProcessedLayers::default(),
        children.into_iter().map(|c| tile(c, true, true)).collect(),
    )
    .expect("partial upload frame");
    assert_deferred(&map, "relief-upload-gap");
    map.map_context
        .world
        .resources
        .get_mut::<faults::Faults>()
        .expect("controls")
        .missing_upload = None;
    map.run_frame().expect("upload completed");
    assert_color(
        &read_blocking(&map, "relief-upload-completed"),
        [8, 8, 136, 255],
    );
}

#[tokio::test]
async fn metadata_shortage_keeps_the_complete_texture_and_retries() {
    let mut map = prepared_map(true).await;
    map.map_context
        .world
        .resources
        .get_mut::<faults::Faults>()
        .expect("controls")
        .metadata_capacity = Some(4);
    map.render_sources(
        ProcessedLayers::default(),
        target()
            .get_children()
            .into_iter()
            .map(|c| tile(c, true, true))
            .collect(),
    )
    .expect("metadata shortage frame");
    assert_deferred(&map, "relief-metadata-deferred");
    map.map_context
        .world
        .resources
        .get_mut::<faults::Faults>()
        .expect("controls")
        .metadata_capacity = None;
    map.run_frame().expect("metadata available");
    assert_color(
        &read_blocking(&map, "relief-metadata-retried"),
        [8, 8, 136, 255],
    );
}

fn assert_deferred(map: &HeadlessMap, name: &str) {
    assert!(
        map.world()
            .resources
            .get::<faults::Faults>()
            .expect("controls")
            .applied
    );
    assert!(
        !map.world()
            .resources
            .get::<crate::terrain::DrapePhase>()
            .expect("phase")
            .targets
            .iter()
            .any(|t| t.coords == target()),
        "incomplete target must not reach the clearing pass"
    );
    assert_color(&read_blocking(map, name), [8, 136, 8, 255]);
}

#[tokio::test]
async fn fallback_keeps_translucent_layers_in_style_order_without_double_blending() {
    let mut map = prepared_layers(true, true).await;
    assert_color(
        &read_blocking(&map, "relief-overlay-parent"),
        [132, 68, 4, 255],
    );
    for (i, child) in target().get_children().into_iter().enumerate() {
        map.render_sources(ProcessedLayers::default(), vec![tile(child, true, true)])
            .expect("child frame");
        assert_color(
            &read_blocking(&map, &format!("relief-overlay-child-{i}")),
            if i == 3 {
                [132, 4, 68, 255]
            } else {
                [132, 68, 4, 255]
            },
        );
    }
}

fn raster_reply(map: &mut HeadlessMap, child: bool, missing: bool) {
    use crate::{
        headless::environment::HeadlessEnvironment,
        io::apc::{tests::reply_context, Context},
        raster::{
            populate_world_system::PopulateWorldSystem, LayerRaster, LayerRasterMissing,
            RasterTransferables,
        },
        tcs::system::System,
    };
    let context = reply_context(map.kernel.apc());
    if missing {
        context
            .send_back(
                <DefaultRasterTransferables as RasterTransferables>::LayerRasterMissing::build_from(
                    target(),
                ),
            )
            .expect("missing worker result");
    } else {
        context
            .send_back(
                <DefaultRasterTransferables as RasterTransferables>::LayerRaster::build_from(
                    target(),
                    "paint".into(),
                    tile(target(), false, child).image,
                ),
            )
            .expect("raster worker result");
    }
    PopulateWorldSystem::<HeadlessEnvironment, DefaultRasterTransferables>::new(&map.kernel)
        .run(&mut map.map_context)
        .expect("worker raster ingestion");
    map.run_frame().expect("raster upload and draw");
}

#[tokio::test]
async fn worker_replacement_refreshes_raster_pixels_without_accumulating_old_images() {
    let mut map = prepared_map(false).await;
    map.map_context
        .world
        .tiles
        .query_mut::<&mut crate::raster::RasterLayersDataComponent>(target())
        .expect("raster data")
        .layers
        .clear();
    raster_reply(&mut map, false, false);
    assert_color(
        &read_blocking(&map, "worker-raster-first"),
        [0, 255, 0, 255],
    );
    raster_reply(&mut map, true, false);
    assert_color(
        &read_blocking(&map, "worker-raster-replaced"),
        [0, 0, 255, 255],
    );
    assert_eq!(
        map.world()
            .tiles
            .query::<&crate::raster::RasterLayersDataComponent>(target())
            .expect("raster data")
            .layers
            .len(),
        1
    );
}

#[tokio::test]
async fn failed_worker_refresh_preserves_the_loaded_raster() {
    let mut map = prepared_map(false).await;
    map.map_context
        .world
        .tiles
        .query_mut::<&mut crate::raster::RasterLayersDataComponent>(target())
        .expect("raster data")
        .layers
        .clear();
    raster_reply(&mut map, false, false);
    raster_reply(&mut map, false, true);
    assert_color(
        &read_blocking(&map, "worker-raster-failed"),
        [0, 255, 0, 255],
    );
    assert!(map
        .world()
        .tiles
        .query::<&crate::raster::RasterLayersDataComponent>(target())
        .expect("raster data")
        .has_image());
}

#[tokio::test]
async fn unrelated_raster_arrival_does_not_redraw_the_cached_target() {
    let mut map = prepared_map(false).await;
    let unrelated = WorldTileCoords {
        x: target().x + 10,
        ..target()
    };
    map.render_sources(
        ProcessedLayers::default(),
        vec![tile(unrelated, false, true)],
    )
    .expect("unrelated raster arrival");
    assert!(!map
        .world()
        .resources
        .get::<crate::terrain::DrapePhase>()
        .expect("phase")
        .targets
        .iter()
        .any(|entry| entry.coords == target()));
    assert_color(&read_blocking(&map, "raster-unrelated"), [0, 255, 0, 255]);
}

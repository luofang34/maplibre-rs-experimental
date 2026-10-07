use super::*;

#[tokio::test]
async fn water_survives_dem_source_children_arriving() {
    for (terrain, samples) in [(true, 1), (true, 4), (false, 1), (false, 4)] {
        let mut map = water_map(terrain, samples).await;
        assert_color(
            &read_stencil_blocking(&map, "stencil-parent"),
            [0, 0, 255, 255],
        );
        map.map_context
            .view_state
            .zoom_to(crate::coords::Zoom::new(12.15));
        for (arrival, child) in target().get_children().into_iter().enumerate() {
            map.render_sources(ProcessedLayers::default(), vec![tile(child, true, false)])
                .expect("child frame");
            assert_color(
                &read_stencil_blocking(&map, &format!("stencil-arrival-{arrival}")),
                [0, 0, 255, 255],
            );
        }
    }
}

#[tokio::test]
async fn child_water_geometry_survives_a_parent_dem_mask() {
    for (terrain, samples) in [(true, 1), (true, 4), (false, 1), (false, 4)] {
        let style = water_style(terrain);
        let mut processed = ProcessedLayers::default();
        for child in target().get_children() {
            processed.append(&mut process(
                &polygon("water", 4096),
                &style.layers[2],
                child,
            ));
        }
        let map = map_with(style, processed, samples, target()).await;
        assert_color(
            &read_stencil_blocking(&map, "stencil-water-children"),
            [0, 0, 255, 255],
        );
    }
}

#[tokio::test]
async fn translucent_layers_keep_style_order_across_stencil_sources() {
    for (terrain, samples) in [(true, 1), (true, 4), (false, 1), (false, 4)] {
        let mut style = water_style(terrain);
        style.layers[2].index = 1;
        style.layers[1].index = 2;
        style.layers.swap(1, 2);
        let processed = process(&polygon("water", 4096), &style.layers[1], target());
        let mut map = map_with(style, processed, samples, target()).await;
        map.render_sources(
            ProcessedLayers::default(),
            target()
                .get_children()
                .into_iter()
                .map(|c| tile(c, true, false))
                .collect(),
        )
        .expect("child frame");
        assert_color(
            &read_stencil_blocking(&map, "stencil-alpha"),
            [0, 128, 127, 255],
        );
    }
}

#[tokio::test]
async fn road_geometry_survives_a_different_dem_zoom() {
    for (terrain, samples) in [(true, 1), (true, 4), (false, 1), (false, 4)] {
        let mut style = water_style(terrain);
        let mut road: StyleLayer = serde_json::from_value(serde_json::json!({
            "id":"road", "type":"line", "source":"water", "source-layer":"road",
            "paint":{"line-color":"#0000ff", "line-width":40}
        }))
        .expect("road layer");
        road.index = 2;
        style.layers[2] = road;
        let bytes = vector_tile("road", vec![9, 0, 4096, 10, 8192, 0], 2);
        let processed = process(&bytes, &style.layers[2], target());
        let mut map = map_with(style, processed, samples, target()).await;
        map.render_sources(
            ProcessedLayers::default(),
            target()
                .get_children()
                .into_iter()
                .map(|c| tile(c, true, false))
                .collect(),
        )
        .expect("child frame");
        let pixels = read_stencil_blocking(&map, "stencil-road");
        for x in 16..SIZE - 16 {
            assert_pixel(&pixels, x, SIZE / 2, [0, 0, 255, 255]);
        }
    }
}

#[tokio::test]
async fn stale_stencil_cannot_admit_buffered_geometry_into_a_distant_tile() {
    for (terrain, samples) in [(true, 1), (true, 4), (false, 1), (false, 4)] {
        let mut style = water_style(terrain);
        let mut overlay: StyleLayer = serde_json::from_value(serde_json::json!({
            "id":"overlay", "type":"fill", "source":"water", "source-layer":"overlay",
            "paint":{"fill-color":"#ff0000"}
        }))
        .expect("overlay layer");
        overlay.index = 3;
        style.layers.push(overlay);
        let grandchildren: Vec<_> = target()
            .get_children()
            .into_iter()
            .flat_map(|c| c.get_children())
            .collect();
        let mut processed = ProcessedLayers::default();
        for child in &grandchildren {
            processed.append(&mut process(
                &polygon("water", 4096),
                &style.layers[2],
                *child,
            ));
        }
        // This bucket's buffer reaches a tile whose parity-based stencil reference is equal.
        processed.append(&mut process(
            &polygon("overlay", 12288),
            &style.layers[3],
            grandchildren[0],
        ));
        let map = map_with(style, processed, samples, target()).await;
        let pixels = read_stencil_blocking(&map, "stencil-buffered");
        assert_pixel(&pixels, 40, 40, [255, 0, 0, 255]);
        for x in [180, 300, 430] {
            assert_pixel(&pixels, x, 40, [0, 0, 255, 255]);
        }
    }
}

#[tokio::test]
async fn root_tile_buffer_cannot_draw_outside_the_world_mask() {
    for samples in [1, 4] {
        let mut style = water_style(false);
        style.center = Some([90.0, 0.0]);
        style.zoom = Some(0.0);
        style.layers.retain(|layer| layer.id == "water");
        let coords = WorldTileCoords::default();
        let reference = map_with(
            style.clone(),
            process(&polygon("water", 4096), &style.layers[0], coords),
            samples,
            coords,
        )
        .await;
        let reference = read_stencil_blocking(&reference, "stencil-root-reference");
        assert_pixel(&reference, 100, SIZE / 2, [0, 0, 255, 255]);
        // Right of the world a copy of it repeats; the buffered tile must not differ from it.
        assert_pixel(&reference, 450, SIZE / 2, [0, 0, 255, 255]);
        let buffered = process(&polygon("water", 12288), &style.layers[0], coords);
        let map = map_with(style, buffered, samples, coords).await;
        let pixels = read_stencil_blocking(&map, "stencil-root-buffered");
        let changed = pixels
            .chunks_exact(4)
            .zip(reference.chunks_exact(4))
            .enumerate()
            .find(|(_, (actual, expected))| {
                actual
                    .iter()
                    .zip(*expected)
                    .any(|(a, b)| a.abs_diff(*b) > 2)
            });
        assert!(
            changed.is_none(),
            "tile buffer escaped the root mask at {changed:?}"
        );
    }
}

#[tokio::test]
async fn globe_water_survives_dem_children_and_border_masks() {
    for samples in [1, 4] {
        let mut style = water_style(false);
        style.center = Some([5.625, -5.615985819155334]);
        style.zoom = Some(5.125);
        style.projection =
            Some(serde_json::from_value(serde_json::json!({"type":"globe"})).expect("projection"));
        let coords = WorldTileCoords::from((16, 16, 5_u8.into()));
        let processed = process_tile_layers(
            &polygon("water", 4096),
            &style.layers[2],
            coords,
            ProjectionType::Globe,
        )
        .expect("globe polygon");
        let mut map = map_with(style, processed, samples, coords).await;
        let before = read_stencil_blocking(&map, "stencil-globe-parent");
        let water: Vec<_> = before
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, rgba)| *rgba == [0, 0, 255, 255])
            .map(|(index, _)| index)
            .collect();
        assert!(water.len() > 4096, "fixture must draw visible globe water");
        for (arrival, child) in coords.get_children().into_iter().enumerate() {
            map.render_sources(ProcessedLayers::default(), vec![tile(child, true, false)])
                .expect("globe DEM child");
            let pixels = read_stencil_blocking(&map, &format!("stencil-globe-child-{arrival}"));
            for index in &water {
                assert_eq!(
                    &pixels[index * 4..index * 4 + 4],
                    &[0, 0, 255, 255],
                    "DEM child {arrival} erased water pixel {index}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_smaller_drape_paints_the_same_ground_in_less_memory() {
    use crate::{
        render::eventually::Eventually::Initialized,
        terrain::resources::{TerrainResources, DRAPE_SIZE},
    };
    let style = water_style(true);
    let processed = process(&polygon("water", 4096), &style.layers[2], target());
    let settings = RendererSettings {
        terrain_drape_size: 512,
        ..Default::default()
    };
    let map = map_with_settings(style, processed, settings, target()).await;
    let Some(Initialized(terrain)) =
        map.map_context
            .world
            .resources
            .get::<crate::render::eventually::Eventually<TerrainResources>>()
    else {
        panic!("terrain resources");
    };
    assert_eq!(terrain.drape_size(), 512);
    let (drapes, _) = terrain.texture_bytes();
    let one = 512 * 512 * 4 * 4 / 3;
    assert!(
        drapes >= one && drapes % one == 0,
        "{drapes} bytes of drapes"
    );
    // A sixteenth of the default drape's texels.
    assert!(one * 15 < (DRAPE_SIZE as usize).pow(2) * 4 * 4 / 3);
    assert_color(
        &read_stencil_blocking(&map, "stencil-small-drape"),
        [0, 0, 255, 255],
    );
}

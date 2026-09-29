use super::*;

#[tokio::test]
async fn water_survives_dem_source_children_arriving() {
    let mut map = water_map().await;
    assert_color(&read_blocking(&map, "stencil-parent"), [0, 0, 255, 255]);
    map.map_context
        .view_state
        .zoom_to(crate::coords::Zoom::new(12.15));
    for (arrival, child) in target().get_children().into_iter().enumerate() {
        map.render_sources(ProcessedLayers::default(), vec![tile(child, true, false)])
            .expect("child frame");
        assert_color(
            &read_blocking(&map, &format!("stencil-arrival-{arrival}")),
            [0, 0, 255, 255],
        );
    }
}

#[tokio::test]
async fn child_water_geometry_survives_a_parent_dem_mask() {
    let style = water_style();
    let mut processed = ProcessedLayers::default();
    for child in target().get_children() {
        processed.append(&mut process(
            &polygon("water", 4096),
            &style.layers[2],
            child,
        ));
    }
    let map = map_with(style, processed).await;
    assert_color(
        &read_blocking(&map, "stencil-water-children"),
        [0, 0, 255, 255],
    );
}

#[tokio::test]
async fn translucent_layers_keep_style_order_across_stencil_sources() {
    let mut style = water_style();
    style.layers[2].index = 1;
    style.layers[1].index = 2;
    style.layers.swap(1, 2);
    let processed = process(&polygon("water", 4096), &style.layers[1], target());
    let mut map = map_with(style, processed).await;
    map.render_sources(
        ProcessedLayers::default(),
        target()
            .get_children()
            .into_iter()
            .map(|c| tile(c, true, false))
            .collect(),
    )
    .expect("child frame");
    assert_color(&read_blocking(&map, "stencil-alpha"), [0, 128, 127, 255]);
}

#[tokio::test]
async fn road_geometry_survives_a_different_dem_zoom() {
    let mut style = water_style();
    let mut road: StyleLayer = serde_json::from_value(serde_json::json!({
        "id":"road", "type":"line", "source":"water", "source-layer":"road",
        "paint":{"line-color":"#0000ff", "line-width":40}
    }))
    .expect("road layer");
    road.index = 2;
    style.layers[2] = road;
    let bytes = vector_tile("road", vec![9, 0, 4096, 10, 8192, 0], 2);
    let processed = process(&bytes, &style.layers[2], target());
    let mut map = map_with(style, processed).await;
    map.render_sources(
        ProcessedLayers::default(),
        target()
            .get_children()
            .into_iter()
            .map(|c| tile(c, true, false))
            .collect(),
    )
    .expect("child frame");
    let pixels = read_blocking(&map, "stencil-road");
    for x in 16..SIZE - 16 {
        assert_pixel(&pixels, x, SIZE / 2, [0, 0, 255, 255]);
    }
}

#[tokio::test]
async fn stale_stencil_cannot_admit_buffered_geometry_into_a_distant_tile() {
    let mut style = water_style();
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
    let map = map_with(style, processed).await;
    let pixels = read_blocking(&map, "stencil-buffered");
    assert_pixel(&pixels, 40, 40, [255, 0, 0, 255]);
    for x in [180, 300, 430] {
        assert_pixel(&pixels, x, 40, [0, 0, 255, 255]);
    }
}

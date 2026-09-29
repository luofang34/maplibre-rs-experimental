use super::*;

#[test]
fn consumed_stencil_references_reject_every_supported_tile() {
    let mut valid = std::collections::HashSet::new();
    for zoom in 0..crate::coords::MAX_ZOOM {
        for x in 0..2 {
            for y in 0..2 {
                let coords = WorldTileCoords::from((x, y, (zoom as u8).into()));
                valid.insert(coords.stencil_reference_value_3d());
            }
        }
    }
    for reference in &valid {
        assert!(
            !valid.contains(&!reference),
            "consumed stencil {reference} matches a supported tile"
        );
    }
}

#[tokio::test]
async fn globe_raster_alpha_draws_interior_only_once() {
    for samples in [1, 4] {
        let map = styled_alpha_map(globe_style(target()), samples, Some(128), true).await;
        assert_spherical(&map);
        assert_center(&pixels_blocking(&map, "globe-raster"), [128, 0, 127, 255]);
    }
}

#[tokio::test]
async fn root_globe_raster_alpha_draws_only_once() {
    for samples in [1, 4] {
        let coords = WorldTileCoords::default();
        let style = with_background(globe_style(coords), "#0000ff");
        let map = styled_alpha_map_at(style, samples, Some(128), false, coords).await;
        assert_spherical(&map);
        assert_center(
            &pixels_blocking(&map, "globe-root-raster"),
            [128, 0, 127, 255],
        );
    }
}

#[tokio::test]
async fn globe_raster_layers_each_compose_once_in_style_order() {
    for samples in [1, 4] {
        let mut style = globe_style(target());
        let mut overlay = style.layers[1].clone();
        overlay.id = "overlay".into();
        overlay.source = Some("overlay".into());
        overlay.index = 2;
        style.sources.insert(
            "overlay".into(),
            style.sources.get("paint").expect("raster source").clone(),
        );
        style.layers.push(overlay);
        let mut map = styled_alpha_map(style, samples, Some(128), true).await;
        map.render_sources(
            ProcessedLayers::default(),
            vec![AvailableRasterLayerData {
                coords: target(),
                source: "overlay".into(),
                image: RgbaImage::from_pixel(256, 256, Rgba([0, 255, 0, 128])),
            }],
        )
        .expect("overlay frame");
        assert_spherical(&map);
        assert_center(
            &pixels_blocking(&map, "globe-raster-layers"),
            [64, 128, 63, 255],
        );
    }
}

#[tokio::test]
async fn globe_raster_children_preserve_seams_and_single_blend() {
    for samples in [1, 4] {
        let coords = WorldTileCoords::from((16, 16, 5_u8.into()));
        let style = globe_style(coords);
        let mut map = styled_alpha_map_at(style, samples, Some(128), true, coords).await;
        assert_spherical(&map);
        assert_color(&pixels_blocking(&map, "globe-parent"), [128, 0, 127, 255]);
        for (arrival, child) in coords.get_children().into_iter().enumerate() {
            map.render_sources(
                ProcessedLayers::default(),
                vec![AvailableRasterLayerData {
                    coords: child,
                    source: "paint".into(),
                    image: RgbaImage::from_pixel(256, 256, Rgba([0, 255, 0, 128])),
                }],
            )
            .expect("child raster frame");
            let pixels = pixels_blocking(&map, &format!("globe-child-{arrival}"));
            assert_source_color(&pixels);
            if arrival == 3 {
                assert_color(&pixels, [0, 128, 127, 255]);
            }
        }
    }
}

use super::*;

#[tokio::test]
async fn constant_dem_has_metric_depth_in_both_projections() {
    for projection in ["mercator", "globe"] {
        for (height, exaggeration) in [(0, 1.0), (1000, 1.0), (-500, 1.0), (1000, 3.0)] {
            let mut harness = Harness::with_exaggeration(projection, height, exaggeration).await;
            for (altitude, fov) in [(4000.0, 60.0), (8000.0, 100.0)] {
                let camera = pose(altitude, 0.0, 0.0);
                let label = format!("known-{projection}-{height}-{exaggeration}-{altitude}");
                let frame = harness.render_blocking(&label, LatLon::new(0.0, 0.0), camera, fov);
                let radius = harness.map.view_state().body().radius_meters;
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        let offset = (y * SIZE + x) as usize;
                        let (origin, direction) = ray(camera, fov, x, y);
                        let expected = if projection == "globe" {
                            sphere_depth(
                                origin,
                                direction,
                                radius,
                                f64::from(height) * exaggeration,
                            )
                            .expect("sphere under camera")
                        } else {
                            altitude - f64::from(height) * exaggeration
                        };
                        assert_eq!(
                            frame.rgba[offset * 4 + 3],
                            255,
                            "{label}: missing terrain ({x},{y})"
                        );
                        assert!(
                            (frame.depth[offset] - expected).abs() < 1.0,
                            "{label} ({x},{y}) depth {} != {expected}",
                            frame.depth[offset]
                        );
                    }
                }
                harness.record(&label, &[frame]);
            }
        }
    }
}

#[tokio::test]
async fn moving_camera_preserves_terrain_coverage() {
    for projection in ["mercator", "globe"] {
        let mut harness = Harness::new(projection, 0).await;
        let anchor = LatLon::new(47.26, 11.39);
        let route = [
            (8000.0, 0.0, 0.0, 60.0),
            (6000.0, 30.0, 30.0, 60.0),
            (4000.0, 60.0, 80.0, 80.0),
            (2000.0, 75.0, 170.0, 90.0),
            (800.0, 85.0, 179.0, 100.0),
            (300.0, 89.0, -179.0, 110.0),
            (80.0, 90.1, -90.0, 120.0),
            (10.0, 110.0, 0.0, 60.0),
        ];
        let mut frames = Vec::new();
        let mut observations = Vec::new();
        let mut results = Vec::new();
        let mut visible_seams = 0;
        for (index, (altitude, pitch, bearing, fov)) in route.into_iter().enumerate() {
            let camera = pose(altitude, pitch, bearing);
            let label = format!("route-{projection}-{index}");
            let frame = harness.render_blocking(&label, anchor, camera, fov);
            let coverage = route::check(
                &frame,
                camera,
                fov,
                anchor,
                projection == "globe",
                harness.map.view_state().body().radius_meters,
            );
            let seam = route::visible_seam(
                &frame,
                camera,
                anchor,
                projection == "globe",
                harness.map.view_state().body().radius_meters,
                fov,
            );
            visible_seams += usize::from(seam.is_some());
            observations.push(serde_json::json!({"name":label,"altitude":altitude,"pitch":pitch,"bearing":bearing,"fov":fov,"coverage":coverage,"unequal_adjacent":route::unequal_adjacent(&frame.tiles),"visible_seam":seam}));
            frames.push(frame);
            harness.record(&format!("route-{projection}"), &frames);
            if let Some(path) = &harness.capture {
                std::fs::write(
                    path.join(format!("route-{projection}-coverage.json")),
                    serde_json::to_vec_pretty(&observations).expect("coverage JSON"),
                )
                .expect("coverage record");
            }
            results.push((label, coverage));
        }
        for (label, coverage) in results {
            route::assert_coverage(&coverage, &label, SIZE as usize * 2);
        }
        assert!(
            visible_seams > 0,
            "route must display an unequal-LOD shared edge inside the frame"
        );
        assert!(
            frames
                .iter()
                .any(|frame| route::unequal_adjacent(&frame.tiles)),
            "route exercises adjacent unequal LOD meshes"
        );
    }
}

#[tokio::test]
async fn terrain_crosses_eastern_antimeridian() {
    boundary(
        "east-dateline",
        "globe",
        LatLon::new(0.0, 179.999),
        (4000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_crosses_western_antimeridian() {
    boundary(
        "west-dateline",
        "globe",
        LatLon::new(0.0, -179.999),
        (4000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_covers_near_pole() {
    boundary(
        "near-pole",
        "globe",
        LatLon::new(89.9, 179.999),
        (10000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_covers_north_pole() {
    boundary(
        "north-pole",
        "globe",
        LatLon::new(90.0, 0.0),
        (4000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_covers_south_pole() {
    boundary(
        "south-pole",
        "globe",
        LatLon::new(-90.0, 0.0),
        (4000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_polar_fan_preserves_visible_surface() {
    boundary(
        "polar-fan",
        "globe",
        LatLon::new(85.051_128_779_806_6, 0.0),
        (1000.0, 60.0, 90.0),
        1024,
    )
    .await;
}

#[tokio::test]
async fn terrain_horizon_from_space_keeps_sky_empty() {
    boundary(
        "space",
        "globe",
        LatLon::new(0.0, 0.0),
        (12_742_017.6, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_low_wide_camera_remains_finite_and_covered() {
    boundary(
        "low-wide",
        "globe",
        LatLon::new(0.0, 0.0),
        (1.0, 85.0, 140.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_mercator_world_edge_matches_canonical_domain() {
    boundary(
        "flat-edge",
        "mercator",
        LatLon::new(85.0, 179.999),
        (1000.0, 0.0, 90.0),
        256,
    )
    .await;
}

async fn boundary(
    name: &str,
    projection: &str,
    anchor: LatLon,
    camera: (f64, f64, f64),
    max_band: usize,
) {
    let (altitude, pitch, fov) = camera;
    let mut harness = Harness::new(projection, 0).await;
    let camera = pose(altitude, pitch, 0.0);
    let frame = harness.render_blocking(name, anchor, camera, fov);
    let coverage = route::check(
        &frame,
        camera,
        fov,
        anchor,
        projection == "globe",
        harness.map.view_state().body().radius_meters,
    );
    if let Some(path) = &harness.capture {
        std::fs::write(path.join(format!("{name}-coverage.json")),serde_json::to_vec_pretty(
            &serde_json::json!({"coverage":coverage,"latitude":anchor.latitude,"longitude":anchor.longitude,"altitude":altitude,"pitch":pitch,"fov":fov})).expect("JSON")).expect("coverage record");
    }
    harness.record(name, &[frame]);
    route::assert_coverage(&coverage, name, max_band);
}

#[tokio::test]
async fn native_orbit_cameras_see_terrain_in_both_projections() {
    for projection in ["mercator", "globe"] {
        let harness = Harness::new(projection, 0).await;
        let rgba = read_texture_blocking(
            &harness.map,
            harness.map.head_texture().expect("color"),
            wgpu::TextureAspect::All,
        )
        .expect("readback");
        harness.capture(&format!("native-{projection}"), &rgba);
        assert!(
            rgba.chunks_exact(4).all(|pixel| pixel[3] == 255),
            "native {projection} terrain is visible throughout the viewport"
        );
    }
}

#[tokio::test]
async fn terrain_mercator_north_edge_does_not_draw_polar_fans() {
    boundary(
        "flat-north-limit",
        "mercator",
        LatLon::new(85.051_128_779_806_6, 0.0),
        (1000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_mercator_south_edge_does_not_draw_polar_fans() {
    boundary(
        "flat-south-limit",
        "mercator",
        LatLon::new(-85.051_128_779_806_6, 0.0),
        (1000.0, 0.0, 90.0),
        256,
    )
    .await;
}

#[tokio::test]
async fn terrain_south_cap_connects_during_projection_transition() {
    transition_cap(-85.051_128_779_806_6).await;
}

#[tokio::test]
async fn terrain_north_cap_connects_during_projection_transition() {
    transition_cap(85.051_128_779_806_6).await;
}

async fn transition_cap(latitude: f64) {
    let mut style = terrain_style("mercator", 1.0);
    style.projection = Some(
        serde_json::from_value(serde_json::json!({
            "type":["mercator","vertical-perspective",0.5]
        }))
        .expect("fixed half transition"),
    );
    let mut harness = Harness::with_style(style, 0).await;
    harness.draw(LatLon::new(latitude, 0.0), pose(4000.0, 0.0, 0.0), 90.0);
    let projection = crate::render::projection::projection_data_for_view(
        &harness.style,
        harness.map.view_state(),
    )
    .expect("projection data");
    assert_eq!(projection.transition, 0.5);
    let rgba = read_texture_blocking(
        &harness.map,
        harness.map.head_texture().expect("color"),
        wgpu::TextureAspect::All,
    )
    .expect("transition color");
    let depth = read_texture_blocking(&harness.map, &harness.depth, wgpu::TextureAspect::DepthOnly)
        .expect("transition depth");
    harness.capture(&format!("half-transition-{latitude}"), &rgba);
    assert!(
        rgba.chunks_exact(4).all(|p| p[3] == 255),
        "projection transition opens the polar seam at {latitude}"
    );
    assert!(
        depth.chunks_exact(4).map(metric_depth).all(|d| d > 0.0),
        "projection transition loses visible terrain depth at {latitude}"
    );
}

#[tokio::test]
async fn terrain_caps_follow_globe_to_flat_transition_in_one_map() {
    for latitude in [-85.051_128_779_806_6, 85.051_128_779_806_6] {
        let mut style = terrain_style("mercator", 1.0);
        style.projection = Some(
            serde_json::from_value(serde_json::json!({"type":"globe"}))
                .expect("zoom-dependent globe"),
        );
        let mut harness = Harness::with_style(style, 0).await;
        let anchor = LatLon::new(latitude, 0.0);
        for (index, (altitude, globe)) in [(10000.0, true), (10.0, false), (10000.0, true)]
            .into_iter()
            .enumerate()
        {
            let camera = pose(altitude, 0.0, 0.0);
            let name = format!("projection-reuse-{latitude}-{index}");
            let frame = harness.render_blocking(&name, anchor, camera, 90.0);
            assert_eq!(frame.globe_transition, f32::from(globe));
            let coverage = route::check(
                &frame,
                camera,
                90.0,
                anchor,
                globe,
                harness.map.view_state().body().radius_meters,
            );
            harness.record(&name, &[frame]);
            route::assert_coverage(&coverage, &name, 256);
        }
    }
}

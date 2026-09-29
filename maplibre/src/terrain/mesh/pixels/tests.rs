use super::*;

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

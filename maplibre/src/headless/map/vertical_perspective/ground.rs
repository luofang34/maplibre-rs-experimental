//! A host reads the ground the map draws, and the ground its loaded DEM describes beyond it.

use super::*;
use crate::coords::LatLon;

#[tokio::test]
async fn a_host_reads_the_drawn_ground_and_the_loaded_dem_from_the_map() {
    let dem = SyntheticDem {
        meters: 1500.0,
        relief: 600.0,
    };
    let scene = Scene {
        meters: dem.meters,
        relief: dem.relief,
        exaggeration: 1.0,
        pitch: 60.0,
        zoom: 11.67,
    };
    let mut level = LevelMap::new(scene).await;
    level.settle(None).await;
    let center = LatLon::new(27.765_393_137_835_165, 88.054_643_590_047_59);
    let mercator = crate::terrain::sightline::lat_lon_to_mercator(center);
    let truth = dem.height(mercator.x, mercator.y);
    let drawn = level
        .map
        .context()
        .drawn_ground_cached(center)
        .expect("the center's DEM has loaded")
        .expect("the frame draws the center");
    let loaded = level
        .map
        .context()
        .loaded_ground_cached(center)
        .expect("the center's DEM has loaded");
    // Both read DEM samples tens of metres apart, the mesh flat between its vertices; over
    // these hills they stay within metres of the ground the samples describe.
    assert!(
        (drawn - truth).abs() < 20.0,
        "drawn at {drawn} m over {truth} m"
    );
    assert!(
        (loaded - truth).abs() < 20.0,
        "loaded at {loaded} m over {truth} m"
    );
    // Beyond the drawn tiles nothing is drawn, though a coarse DEM may describe it.
    assert_eq!(
        level
            .map
            .context()
            .drawn_ground_cached(LatLon::new(-40.0, -100.0)),
        Ok(None)
    );
}

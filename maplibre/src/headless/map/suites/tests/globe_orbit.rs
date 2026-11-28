//! An orbit over the globe lists an unchanged covering in a new order, nearest the focus
//! first. The tile pattern is the same set of tiles either way and must not be rebuilt for it.

use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    map::RenderPlugin,
    render::{
        eventually::Eventually, eye_covering::SharedCovering,
        tile_view_pattern::WgpuTileViewPattern, view_state::CameraOptions,
    },
    style::Style,
};

fn covering(map: &HeadlessMap) -> Vec<WorldTileCoords> {
    map.world()
        .resources
        .get::<SharedCovering>()
        .and_then(SharedCovering::tiles)
        .map(<[WorldTileCoords]>::to_vec)
        .unwrap_or_default()
}

fn topology(map: &HeadlessMap) -> u64 {
    match map
        .world()
        .resources
        .get::<Eventually<WgpuTileViewPattern>>()
    {
        Some(Eventually::Initialized(pattern)) => pattern.topology_generation(),
        _ => panic!("the tile pattern is initialized after a frame"),
    }
}

#[test]
fn an_orbit_that_only_reorders_the_covering_keeps_the_pattern_topology() {
    crate::headless::map::test_async::run_async(async {
        let style: Style = serde_json::from_value(serde_json::json!({
            "version": 8, "center": [0.0, 10.0], "zoom": 1.2,
            "projection": {"type": "globe"},
            "sources": {},
            "layers": [{"id": "background", "type": "background",
                "paint": {"background-color": "#102030"}}]
        }))
        .expect("style");
        let (kernel, renderer) = create_headless_renderer(512, 512, None)
            .await
            .expect("renderer");
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![Box::new(RenderPlugin), Box::new(HeadlessPlugin::new(false))],
        )
        .expect("map");
        for _ in 0..3 {
            map.run_frame().expect("frame");
        }

        let mut previous = covering(&map);
        let mut generation = topology(&map);
        let mut reorders = 0;
        for step in 1..=80 {
            map.jump_to(CameraOptions {
                center: Some(LatLon {
                    latitude: 10.0,
                    longitude: f64::from(step) * 1.5,
                }),
                ..Default::default()
            })
            .expect("jump");
            map.run_frame().expect("frame");
            let current = covering(&map);
            let (mut before, mut after) = (previous.clone(), current.clone());
            before.sort();
            after.sort();
            if before == after && previous != current {
                reorders += 1;
                assert_eq!(
                    topology(&map),
                    generation,
                    "step {step}: a reordered covering rebuilt the pattern"
                );
            }
            generation = topology(&map);
            previous = current;
        }
        assert!(
            reorders > 0,
            "the orbit never reordered an unchanged covering"
        );
    })
}

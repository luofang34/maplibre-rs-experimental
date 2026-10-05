//! Where provided shields stand and what making them costs: a full queue, a bannered body on
//! its point, the globe, and tiles panned into view.

use super::*;

#[tokio::test]
async fn a_full_generation_queue_draws_every_shield_in_the_end() {
    let (shields, gate) = Shields::held(Answer::Shield(SHIELD));
    let server = AssetServer::default();
    // Each tile column carries a route of its own, so the view asks for several at once.
    for x in 8188..8197 {
        server.serve(
            &format!("https://tiles.test/14/{x}/"),
            road_tile("US:I", &format!("{x}")),
        );
    }
    let mut map = SymbolMap::serving(style("line", false), server).await;
    let providers = map.map.image_providers().expect("registry");
    providers.set_limits(ProviderLimits {
        max_running: 1,
        max_waiting: 0,
        ..ProviderLimits::default()
    });
    providers.register("shield", shields.clone());
    // One generator runs and the queue has no room, so the other routes are refused.
    for _ in 0..600 {
        if providers.stats().refused > 0 {
            break;
        }
        map.frame().await;
    }
    let refused = providers.stats();
    assert!(refused.refused > 0, "the queue fills: {refused:?}");
    assert_eq!(refused.running, 1, "{refused:?}");
    gate.add_permits(1024);
    let pixels = frames_until(&mut map, "every shield", |pixels| {
        shown(pixels, SHIELD) > 300 && shown(pixels, MARKER) == 0
    })
    .await;
    assert_eq!(shown(&pixels, MARKER), 0);
    let stats = providers.stats();
    assert_eq!(stats.running + stats.waiting, 0, "{stats:?}");
    assert!(stats.images >= 3, "{stats:?}");
}

#[tokio::test]
async fn a_bannered_shield_sits_on_its_point_by_its_body() {
    let shields = Shields::new(Answer::Bannered);
    let mut map = shield_map("point", shields).await;
    let pixels = map.settle().await;
    let body = count(&pixels, SHIELD, WHOLE);
    let banner = count(&pixels, BANNER, WHOLE);
    assert!(
        body.len() > 300 && banner.len() > 100,
        "{} {}",
        body.len(),
        banner.len()
    );
    let centre = |pixels: &[[u32; 2]]| {
        let n = pixels.len() as f64;
        [
            pixels.iter().map(|[x, _]| f64::from(*x) + 0.5).sum::<f64>() / n,
            pixels.iter().map(|[_, y]| f64::from(*y) + 0.5).sum::<f64>() / n,
        ]
    };
    let [x, y] = centre(&body);
    let middle = f64::from(SIZE) / 2.0;
    assert!(
        (x - middle).abs() < 1.0 && (y - middle).abs() < 1.0,
        "the body's centre ({x}, {y}) is the point's ({middle}, {middle})"
    );
    assert!(
        centre(&banner)[1] < y - 10.0,
        "the banner is above the body"
    );
}

#[tokio::test]
async fn shields_made_on_request_draw_on_the_globe() {
    let shields = Shields::new(Answer::Shield(SHIELD));
    let mut map = shield_map_on("line", true, shields).await;
    let pixels = map.settle().await;
    assert!(shown(&pixels, SHIELD) > 300);
    assert_eq!(shown(&pixels, MARKER), 0);
}

#[tokio::test]
async fn tiles_panned_into_view_draw_known_shields_without_making_them_again() {
    let shields = Shields::new(Answer::Shield(SHIELD));
    let mut map = shield_map("line", shields.clone()).await;
    map.settle().await;
    let providers = map.map.image_providers().expect("registry");
    let before = providers.stats();
    let fetched = map.server.requested().len();
    // Pan two tiles east in small steps; new tiles arrive along the way.
    for _ in 0..64 {
        map.map
            .view_state_mut()
            .camera_mut()
            .move_relative(cgmath::Vector2::new(16.0, 0.0));
        map.frame().await;
        let pixels = map.read();
        assert_eq!(
            shown(&pixels, MARKER),
            0,
            "a tile arriving with a known shield never shows the fallback"
        );
    }
    map.settle().await;
    let after = providers.stats();
    assert!(map.server.requested().len() > fetched, "new tiles arrived");
    assert_eq!(
        after.calls, before.calls,
        "no shield is made again: {after:?}"
    );
    assert_eq!(
        after.relaid, before.relaid,
        "a known shield is packed when the tile is first laid out: {after:?}"
    );
    assert!(after.cache_hits > before.cache_hits, "{after:?}");
}

#[tokio::test]
async fn a_tile_leaving_the_view_stops_making_its_shield() {
    let (shields, _gate) = Shields::held(Answer::Shield(SHIELD));
    let server = AssetServer::default();
    // Roads only around the start, so the view panned away asks for no shield.
    for x in 8188..8197 {
        server.serve(
            &format!("https://tiles.test/14/{x}/"),
            road_tile("US:I", "287"),
        );
    }
    let mut map = SymbolMap::serving(style("line", false), server).await;
    let providers = map.map.image_providers().expect("registry");
    providers.register("shield", shields.clone());
    frames_until(&mut map, "the fallback", |pixels| {
        shown(pixels, MARKER) > 100
    })
    .await;
    assert_eq!(providers.stats().running, 1, "the shield is being made");
    // Far east, where no tile carries a road.
    map.map
        .view_state_mut()
        .camera_mut()
        .move_relative(cgmath::Vector2::new(20_000.0, 0.0));
    for _ in 0..120 {
        map.frame().await;
        if providers.stats().running == 0 {
            break;
        }
    }
    let stats = providers.stats();
    assert_eq!(
        (stats.running, stats.cancelled),
        (0, 1),
        "no tile in view waits for the shield, so its making stops: {stats:?}"
    );
}

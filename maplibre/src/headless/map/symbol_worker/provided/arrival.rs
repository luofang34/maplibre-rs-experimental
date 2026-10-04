//! A shield's way from request to screen: pending, ready, absent, failed or unavailable for
//! now, made for the display's density, and never applied to a style that changed meanwhile.

use std::sync::atomic::Ordering;

use super::*;

#[tokio::test]
async fn a_shield_made_on_request_replaces_its_fallback_when_it_arrives() {
    let (shields, gate) = Shields::held(Answer::Shield(SHIELD));
    let mut map = shield_map("line", shields.clone()).await;
    for _ in 0..60 {
        watched_frame(&mut map, &shields).await;
    }
    let pixels = frames_until(&mut map, "the fallback", |pixels| {
        shown(pixels, MARKER) > 100
    })
    .await;
    assert_eq!(shown(&pixels, SHIELD), 0, "the shield is still being made");
    assert!(
        map.map.needs_redraw(),
        "frames keep coming while it is made"
    );
    let fetched = map.server.requested().len();
    gate.add_permits(64);
    for _ in 0..60 {
        watched_frame(&mut map, &shields).await;
    }
    let pixels = frames_until(&mut map, "the shield", |pixels| {
        shown(pixels, SHIELD) > 300 && shown(pixels, MARKER) == 0
    })
    .await;
    assert!(shown(&pixels, SHIELD) > 300);
    let settled = map.settle().await;
    assert_eq!(
        shown(&settled, MARKER),
        0,
        "the shield replaced every fallback"
    );
    assert_eq!(
        map.server.requested().len(),
        fetched,
        "the arrival lays labels out again from tiles already held"
    );
    let stats = map.map.image_providers().expect("registry").stats();
    assert_eq!(
        shields.calls(),
        1,
        "one route is made once however many tiles show it: {stats:?}"
    );
    let tiles = map
        .server
        .requested()
        .iter()
        .filter(|url| url.starts_with("https://tiles.test/"))
        .count();
    assert!(
        stats.relaid >= 1 && stats.relaid as usize <= tiles,
        "only the tiles that drew the route lay their labels out again, once: {stats:?}"
    );
    assert_eq!(
        shields.during_frames.load(Ordering::SeqCst),
        0,
        "shields are made by tile workers between frames, never while one is drawn"
    );
}

#[tokio::test]
async fn a_route_without_a_shield_keeps_its_fallback() {
    for answer in [Answer::Absent, Answer::Failed] {
        let shields = Shields::new(answer);
        let mut map = shield_map("line", shields.clone()).await;
        let pixels = map.settle().await;
        assert!(
            shown(&pixels, MARKER) > 100,
            "{answer:?}: the fallback is drawn"
        );
        assert_eq!(shown(&pixels, SHIELD), 0, "{answer:?}");
        assert_eq!(shields.calls(), 1, "{answer:?}: the answer is remembered");
        let stats = map.map.image_providers().expect("registry").stats();
        let counted = if answer == Answer::Absent {
            stats.absent
        } else {
            stats.failed
        };
        assert_eq!(counted, 1, "{answer:?}: {stats:?}");
    }
}

#[tokio::test]
async fn a_provider_unavailable_for_now_is_asked_again_later() {
    let shields = Shields::new(Answer::UnavailableFor(2));
    let mut map = shield_map("point", shields.clone()).await;
    let pixels = frames_until(&mut map, "the shield after retries", |pixels| {
        shown(pixels, SHIELD) > 300
    })
    .await;
    assert_eq!(shown(&pixels, MARKER), 0);
    assert_eq!(shields.calls(), 3);
    assert_eq!(
        map.map
            .image_providers()
            .expect("registry")
            .stats()
            .unavailable,
        2
    );
}

/// The height in physical pixels of the provided shield nearest the viewport's centre.
fn shield_side(pixels: &[u8]) -> u32 {
    let shield = count(pixels, SHIELD, WHOLE);
    let top = shield.iter().map(|[_, y]| *y).min().expect("a shield");
    let bottom = shield.iter().map(|[_, y]| *y).max().expect("a shield");
    bottom - top + 1
}

#[tokio::test]
async fn shields_are_made_for_the_display_pixel_ratio() {
    let mut sides = Vec::new();
    for ratio in [1.0, 2.0, 3.0] {
        let shields = Shields::new(Answer::Shield(SHIELD));
        let mut map = shield_map("point", shields.clone()).await;
        map.map.set_pixel_ratio(ratio);
        let pixels = map.settle().await;
        assert_eq!(shields.pixel_ratios(), [ratio as f32]);
        sides.push(shield_side(&pixels));
    }
    for (side, ratio) in sides.iter().zip([1.0, 2.0, 3.0]) {
        let expected = f64::from(SIDE) * ratio;
        assert!(
            (f64::from(*side) - expected).abs() <= 2.0,
            "a {SIDE}-pixel shield is {expected} device pixels at {ratio}x: {sides:?}"
        );
    }
    // Moving to a denser display makes the shield again for it, at the same layout size.
    let shields = Shields::new(Answer::Shield(SHIELD));
    let mut map = shield_map("point", shields.clone()).await;
    map.settle().await;
    map.map.set_pixel_ratio(2.0);
    let pixels = map.settle().await;
    assert_eq!(shields.pixel_ratios(), [1.0, 2.0]);
    let side = shield_side(&pixels);
    assert!((f64::from(side) - 40.0).abs() <= 2.0, "{side}");
}

#[tokio::test]
async fn a_shield_arriving_after_its_layer_changed_is_not_drawn() {
    let (shields, gate) = Shields::held(Answer::Shield(SHIELD));
    let mut map = shield_map("point", shields.clone()).await;
    frames_until(&mut map, "the fallback", |pixels| {
        shown(pixels, MARKER) > 100
    })
    .await;
    map.map
        .mutate_style(|style| {
            style.set_layout_property("shield", "icon-image", serde_json::json!("marker"))
        })
        .expect("layout change");
    frames_until(&mut map, "the new layout", |pixels| {
        shown(pixels, MARKER) > 100
    })
    .await;
    gate.add_permits(64);
    for _ in 0..60 {
        map.frame().await;
        let pixels = map.read();
        assert_eq!(
            shown(&pixels, SHIELD),
            0,
            "a shield made for the old layout must not reach the new one"
        );
    }
}

#[tokio::test]
async fn invalidating_a_namespace_redraws_its_shields_from_the_new_pack() {
    let shields = Shields::new(Answer::Shield(SHIELD));
    let mut map = shield_map("line", shields.clone()).await;
    let pixels = map.settle().await;
    assert!(shown(&pixels, SHIELD) > 300);
    *shields.answer.lock().expect("answer") = Answer::Shield(NEW_SHIELD);
    *shields.generation.lock().expect("generation") = "pack-2".into();
    let tiles = map.map.invalidate_provided_images("shield");
    assert!(
        tiles >= 1,
        "the tiles that drew a shield are requested again"
    );
    let pixels = frames_until(&mut map, "the new pack's shield", |pixels| {
        shown(pixels, NEW_SHIELD) > 300 && shown(pixels, SHIELD) == 0
    })
    .await;
    assert!(shown(&pixels, NEW_SHIELD) > 300);
    assert_eq!(shields.calls(), 2);
    assert_eq!(map.map.invalidate_provided_images("badge"), 0);
}

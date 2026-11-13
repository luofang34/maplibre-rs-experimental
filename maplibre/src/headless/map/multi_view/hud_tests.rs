//! The navigation HUD belongs to the map's own view; a view slot draws it only when asked to.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use super::*;
use crate::{
    coords::{LatLon, Zoom},
    hud::NavigationHudState,
    render::camera::EdgeInsets,
    window::PhysicalSize,
};

const MAIN: (u32, u32) = (256, 256);
/// Narrower than the map's own view, so the capsule sits at a different place in each.
const SLOT: (u32, u32) = (192, 240);

async fn map_with_published_hud() -> HeadlessMap {
    let style: crate::style::Style = serde_json::from_str(
        r##"{"version":8,"center":[10,40],"zoom":3,"sources":{},
            "layers":[{"id":"bg","type":"background","paint":{"background-color":"#000000"}}]}"##,
    )
    .expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(MAIN.0, MAIN.1, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(crate::render::RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::hud::NavigationHudPlugin),
        ],
    )
    .expect("map");
    let mut controls = crate::navigation::NavigationState::default();
    map.navigate(&mut controls, Duration::from_millis(16));
    map
}

/// Sum of the colour inside the capsule's zoom buttons, were the capsule drawn for a view of
/// `size`, over a background that is black.
fn capsule(map: &HeadlessMap, texture: &wgpu::Texture, size: (u32, u32)) -> u64 {
    let pixels =
        crate::headless::map::readback::read_blocking(map, texture, wgpu::TextureAspect::All)
            .expect("readback");
    let origin = NavigationHudState::panel_origin(
        f64::from(size.0),
        f64::from(size.1),
        &EdgeInsets::default(),
    );
    let (x0, y0) = (origin.x as usize + 8, origin.y as usize + 80);
    (y0..y0 + 40)
        .flat_map(|y| (x0..x0 + 20).map(move |x| (y * size.0 as usize + x) * 4))
        .map(|i| pixels[i..i + 3].iter().map(|&c| u64::from(c)).sum::<u64>())
        .sum()
}

fn draw_slot(map: &mut HeadlessMap, slot: ViewSlotId) -> u64 {
    map.run_multi_view_frame(MultiViewFrame {
        slots: vec![ViewSlotEntry {
            slot,
            view: SlotView::Camera {
                center: LatLon::new(40.0, 10.0),
                zoom: Zoom::new(3.0),
                bearing_deg: 0.0,
                pitch_deg: 0.0,
            },
            policy: CoveringPolicy::Independent,
        }],
        ..Default::default()
    })
    .expect("slot frame");
    let texture = map.view_slot_color_texture(slot).expect("slot colour");
    capsule(map, texture, SLOT)
}

fn draw_main(map: &mut HeadlessMap) -> u64 {
    map.run_frame().expect("frame");
    capsule(map, map.head_texture().expect("head texture"), MAIN)
}

#[test]
fn a_view_slot_draws_the_hud_only_when_asked_to() {
    crate::headless::map::test_async::run_async(async {
        let mut map = map_with_published_hud().await;
        let slot = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(SLOT.0, SLOT.1).expect("size"),
            depth_readout: false,
        });

        assert!(draw_main(&mut map) > 0, "the map's own view has the HUD");
        assert_eq!(draw_slot(&mut map, slot), 0, "a slot has none by default");
        assert!(
            draw_main(&mut map) > 0,
            "the map's own view keeps the HUD after a frame of slots"
        );

        map.set_view_slot_navigation_hud(slot, true).expect("slot");
        assert!(
            draw_slot(&mut map, slot) > 0,
            "a slot asked to draws the HUD where its own size puts it"
        );
        assert!(draw_main(&mut map) > 0);

        map.set_view_slot_navigation_hud(slot, false).expect("slot");
        assert_eq!(draw_slot(&mut map, slot), 0);
    })
}

#[test]
fn the_hud_setting_of_an_unregistered_slot_is_refused() {
    crate::headless::map::test_async::run_async(async {
        let mut map = map_with_published_hud().await;
        let slot = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(SLOT.0, SLOT.1).expect("size"),
            depth_readout: false,
        });
        map.set_view_slot_navigation_hud(slot, true).expect("slot");
        assert!(map.remove_view_slot(slot));
        assert!(matches!(
            map.set_view_slot_navigation_hud(slot, true),
            Err(MultiViewError::UnknownSlot { .. })
        ));

        // A slot registered afterwards starts without the HUD, whatever the removed one had.
        let next = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(SLOT.0, SLOT.1).expect("size"),
            depth_readout: false,
        });
        assert_eq!(draw_slot(&mut map, next), 0);
    })
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc};

use futures::StreamExt;

use super::{MapErrorKind, MapEvent, MapEventBus};

#[test]
fn event_bus_delivers_to_subscribers_and_callbacks() {
    let bus = MapEventBus::new();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    let recorded = Rc::new(RefCell::new(Vec::new()));
    let recorded_clone = Rc::clone(&recorded);
    let listener_id = bus.on_event(move |event| {
        recorded_clone.borrow_mut().push(event.clone());
    });

    bus.emit(MapEvent::StyleDataLoaded);
    bus.emit(MapEvent::SourceDataLoaded {
        source: "roads".into(),
    });
    bus.emit_error(MapErrorKind::Style, "invalid paint");

    let expected = vec![
        MapEvent::StyleDataLoaded,
        MapEvent::SourceDataLoaded {
            source: "roads".into(),
        },
        MapEvent::Error {
            kind: MapErrorKind::Style,
            message: "invalid paint".into(),
        },
    ];
    assert_eq!(rx1.drain(), expected);
    assert_eq!(rx2.drain(), expected);
    assert_eq!(*recorded.borrow(), expected);

    assert!(bus.remove_listener(listener_id));
    assert!(!bus.remove_listener(listener_id));

    bus.emit(MapEvent::MapIdle);
    assert_eq!(rx1.try_recv(), Some(MapEvent::MapIdle));
    assert_eq!(recorded.borrow().len(), 3);
}

#[test]
fn missing_image_deduplicates_until_cleared_or_reset() {
    let bus = MapEventBus::new();
    let mut rx = bus.subscribe();

    assert!(bus.emit_missing_image("custom-pin"));
    assert!(!bus.emit_missing_image("custom-pin"));
    assert!(!bus.emit_missing_image(""));
    assert_eq!(
        rx.drain(),
        vec![MapEvent::StyleImageMissing {
            id: "custom-pin".into()
        }]
    );

    bus.clear_missing_image("custom-pin");
    assert!(bus.emit_missing_image("custom-pin"));
    assert_eq!(
        rx.drain(),
        vec![MapEvent::StyleImageMissing {
            id: "custom-pin".into()
        }]
    );

    bus.reset_style_state();
    assert!(bus.emit_missing_image("custom-pin"));
    assert_eq!(
        rx.drain(),
        vec![MapEvent::StyleImageMissing {
            id: "custom-pin".into()
        }]
    );
}

#[test]
fn receiver_next_and_stream_yield_events_in_order() {
    let bus = MapEventBus::new();
    let mut rx = bus.subscribe();
    bus.emit(MapEvent::MapLoaded);
    bus.emit(MapEvent::MapIdle);

    futures::executor::block_on(async {
        assert_eq!(rx.next().await, Some(MapEvent::MapLoaded));
        assert_eq!(StreamExt::next(&mut rx).await, Some(MapEvent::MapIdle));
    });
}

#[cfg(all(
    feature = "headless",
    feature = "geojson",
    feature = "thread-safe-futures"
))]
mod headless_tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        background::BackgroundPlugin,
        headless::{
            create_headless_renderer_with_loader,
            map::{
                readback::{FrameReadback, PixelLayout},
                HeadlessMap,
            },
        },
        io::{
            resource_loader::SharedLoader,
            source_client::{HttpClient, SourceFetchError},
        },
        render::{settings::RendererSettings, RenderPlugin},
        sdf::{SdfPlugin, SymbolLayersDataComponent},
        style::{Style, StyleImage},
        vector::{DefaultVectorTransferables, VectorPlugin},
    };

    #[derive(Clone, Default)]
    struct EmptyHttpClient;

    #[async_trait::async_trait]
    impl HttpClient for EmptyHttpClient {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
            Err(SourceFetchError::not_found(url))
        }
    }

    async fn build_headless_map(style_json: serde_json::Value) -> HeadlessMap {
        let style: Style = serde_json::from_value(style_json).expect("valid style");
        let loader = SharedLoader::new(EmptyHttpClient);
        let (kernel, renderer) =
            create_headless_renderer_with_loader(128, 128, RendererSettings::default(), loader)
                .await
                .expect("headless renderer");
        HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
                Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
            ],
        )
        .expect("headless map")
    }

    async fn step_frame(map: &mut HeadlessMap) {
        map.frame_input_mut().timestamp += std::time::Duration::from_millis(16);
        map.run_frame().expect("frame succeeds");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn lifecycle_style_and_source_events_emit_cleanly() {
        let mut map = build_headless_map(serde_json::json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 4.0,
            "sources": {
                "pts": {
                    "type": "geojson",
                    "data": {
                        "type": "FeatureCollection",
                        "features": [{
                            "type": "Feature",
                            "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                            "properties": {}
                        }]
                    }
                }
            },
            "layers": [
                { "id": "bg", "type": "background", "paint": { "background-color": "#112233" } },
                { "id": "dot", "type": "circle", "source": "pts", "paint": { "circle-radius": 8.0, "circle-color": "#ff0000" } }
            ]
        }))
        .await;

        let mut rx = map.subscribe_events();
        let callback_events: Arc<Mutex<Vec<MapEvent>>> = Arc::default();
        let cb_clone = Arc::clone(&callback_events);
        map.on_event(move |ev| {
            cb_clone.lock().expect("lock").push(ev.clone());
        });

        for _ in 0..120 {
            step_frame(&mut map).await;
            if !map.needs_redraw() {
                break;
            }
        }

        let initial = rx.drain();
        assert!(
            initial.contains(&MapEvent::StyleDataLoaded),
            "expected initial StyleDataLoaded in {initial:?}"
        );
        assert!(
            initial.contains(&MapEvent::SourceDataLoaded {
                source: "pts".into()
            }),
            "expected SourceDataLoaded(pts) in {initial:?}"
        );
        assert_eq!(
            initial
                .iter()
                .filter(|ev| matches!(ev, MapEvent::MapLoaded))
                .count(),
            1,
            "MapLoaded must be emitted once: {initial:?}"
        );
        assert!(
            initial.contains(&MapEvent::MapIdle),
            "expected MapIdle once frame settled: {initial:?}"
        );

        // Running an extra unrequested frame while already settled must not re-emit MapIdle or MapLoaded.
        step_frame(&mut map).await;
        assert!(
            rx.drain().is_empty(),
            "settled map must not re-emit idle on no-op frame"
        );

        // Invalid style mutation emits MapEvent::Error(Style).
        let err = map.mutate_style(|style| {
            style.set_paint_property("nonexistent", "circle-radius", serde_json::json!(12.0))
        });
        assert!(err.is_err());
        let err_events = rx.drain();
        assert!(
            err_events.iter().any(|ev| matches!(
                ev,
                MapEvent::Error {
                    kind: MapErrorKind::Style,
                    ..
                }
            )),
            "expected Style error event in {err_events:?}"
        );

        // Valid style mutation + GeoJSON update emit StyleDataLoaded, SourceDataLoaded, and MapIdle on settle.
        map.mutate_style(|style| {
            style.set_paint_property("bg", "background-color", serde_json::json!("#224466"))
        })
        .expect("paint update");
        map.set_geojson_source(
            "pts",
            serde_json::json!({
                "type": "FeatureCollection",
                "features": [{
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [0.01, 0.01] },
                    "properties": {}
                }]
            }),
        )
        .expect("geojson update");

        for _ in 0..120 {
            step_frame(&mut map).await;
            if !map.needs_redraw() {
                break;
            }
        }
        let post_mutation = rx.drain();
        assert!(post_mutation.contains(&MapEvent::StyleDataLoaded));
        assert!(post_mutation.contains(&MapEvent::SourceDataLoaded {
            source: "pts".into()
        }));
        assert!(post_mutation.contains(&MapEvent::MapIdle));
        assert!(
            !post_mutation.contains(&MapEvent::MapLoaded),
            "MapLoaded must not fire a second time"
        );
        assert!(!callback_events.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn style_image_missing_dynamically_registers_icon_into_symbol_atlas() {
        let mut map = build_headless_map(serde_json::json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 6.0,
            "sources": {
                "pins": {
                    "type": "geojson",
                    "data": {
                        "type": "FeatureCollection",
                        "features": [{
                            "type": "Feature",
                            "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                            "properties": { "icon": "dynamic-pin" }
                        }]
                    }
                }
            },
            "layers": [
                { "id": "bg", "type": "background", "paint": { "background-color": "#000000" } },
                {
                    "id": "pin-layer",
                    "type": "symbol",
                    "source": "pins",
                    "layout": {
                        "icon-image": ["get", "icon"],
                        "icon-allow-overlap": true,
                        "icon-ignore-placement": true
                    }
                }
            ]
        }))
        .await;

        let mut rx = map.subscribe_events();
        let mut missing_seen = Vec::new();
        for _ in 0..150 {
            step_frame(&mut map).await;
            for ev in rx.drain() {
                if let MapEvent::StyleImageMissing { id } = ev {
                    missing_seen.push(id);
                }
            }
            if !missing_seen.is_empty() && !map.needs_redraw() {
                break;
            }
        }

        // Extra frames do not duplicate StyleImageMissing for the same id.
        step_frame(&mut map).await;
        for ev in rx.drain() {
            if let MapEvent::StyleImageMissing { id } = ev {
                missing_seen.push(id);
            }
        }
        assert_eq!(missing_seen, vec!["dynamic-pin".to_string()]);

        // Host dynamically generates and supplies a bright green 16x16 RGBA icon.
        let side = 16_u32;
        let green_icon = StyleImage {
            width: side,
            height: side,
            data: [0_u8, 255, 0, 255].repeat((side * side) as usize),
            pixel_ratio: 1.0,
            sdf: false,
        };
        map.add_style_image("dynamic-pin", green_icon)
            .expect("add_style_image succeeds");

        for _ in 0..200 {
            step_frame(&mut map).await;
            let has_icon_in_atlas = map.world().tiles.tiles.values().any(|tile| {
                map.world()
                    .tiles
                    .query::<&SymbolLayersDataComponent>(tile.coords)
                    .is_some_and(|comp| {
                        comp.layers.iter().any(|layer| {
                            layer
                                .atlas
                                .as_ref()
                                .is_some_and(|atlas| atlas.icons.contains_key("dynamic-pin"))
                        })
                    })
            });
            if has_icon_in_atlas && !map.needs_redraw() {
                break;
            }
        }

        // Verify the dynamically registered icon entered the tile's SymbolAtlas!
        let packed_entry = map
            .world()
            .tiles
            .tiles
            .values()
            .find_map(|tile| {
                let comp = map
                    .world()
                    .tiles
                    .query::<&SymbolLayersDataComponent>(tile.coords)?;
                comp.layers.iter().find_map(|layer| {
                    layer
                        .atlas
                        .as_ref()
                        .and_then(|atlas| atlas.icons.get("dynamic-pin").cloned())
                })
            })
            .expect("dynamic-pin must be packed into SymbolAtlas");
        assert_eq!(packed_entry.rect[2], 16);
        assert_eq!(packed_entry.rect[3], 16);

        // Run a couple more frames so symbol fade reaches full opacity, then verify green pixels rendered!
        for _ in 0..25 {
            step_frame(&mut map).await;
        }
        let mut readback = FrameReadback::new(&map, 128, 128);
        let mut rgba = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
        readback
            .read_into_blocking(
                &map,
                map.head_texture().expect("head texture"),
                PixelLayout::Rgba8,
                &mut rgba,
            )
            .expect("readback succeeds");
        let green_pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 40 && px[1] > 200 && px[2] < 40)
            .count();
        assert!(
            green_pixels > 0,
            "expected dynamically added green icon to render onto frame"
        );
    }
}

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
